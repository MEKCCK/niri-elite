//! Built-in xdg-desktop-portal backend.
//!
//! Exports org.freedesktop.impl.portal.ScreenCast and Screenshot on the
//! org.freedesktop.impl.portal.desktop.niri bus name, so that the
//! xdg-desktop-portal broker can talk to niri directly without a separate
//! backend process.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use futures_util::future::{select, Either};
use futures_util::StreamExt;
use zbus::fdo::{self, RequestNameFlags};
use zbus::message::Header;
use zbus::names::{OwnedUniqueName, UniqueName};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{
    DeserializeDict, NoneValue, OwnedObjectPath, OwnedValue, SerializeDict, Type, Value,
};
use zbus::{interface, ObjectServer};

use super::gnome_shell_screenshot::{file_uri, ScreenshotToNiri};
use super::mutter_screen_cast::{CursorMode, NodeIdSink, ScreenCastToNiri, StreamTargetId};
use super::niri_portal_screen_cast::{
    validate_selection, PickSourcesReply, PickSourcesRequest, PickerCursorMode, PickerOptions,
    PickerPersistMode, PickerRequestId, PickerSourceTypes,
};
use crate::backend::IpcOutputMap;
use crate::ui::screenshot_ui::ScreenshotPortalError;
use crate::utils::{CastSessionId, CastStreamId};

const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.niri";

const RESPONSE_SUCCESS: u32 = 0;
const RESPONSE_CANCELLED: u32 = 1;
const RESPONSE_ERROR: u32 = 2;

const SOURCE_TYPE_MONITOR: u32 = 1;
const SOURCE_TYPE_WINDOW: u32 = 2;

const RESTORE_FORMAT_VERSION: u32 = 1;
/// Vendors whose restore data we can parse. Both use the same inner format.
const RESTORE_VENDORS: [&str; 2] = ["SHORINNIRI", "GNOME"];
/// Vendor we serialize restore data as, for compatibility with data persisted
/// by xdg-desktop-portal-shorinniri.
const RESTORE_VENDOR: &str = "SHORINNIRI";

/// Maps active cast sessions to their impl.portal.Session object paths.
///
/// Shared with [`crate::niri::Niri::stop_cast`] so compositor-initiated stops
/// close the corresponding portal session.
pub type PortalCastMap = Arc<Mutex<HashMap<CastSessionId, OwnedObjectPath>>>;

struct Shared {
    to_niri_cast: calloop::channel::Sender<ScreenCastToNiri>,
    to_niri_screenshot: calloop::channel::Sender<ScreenshotToNiri>,
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    sessions: Mutex<HashMap<OwnedObjectPath, SessionState>>,
    cast_paths: PortalCastMap,
    session_counter: AtomicU64,
}

struct SessionState {
    owner: OwnedUniqueName,
    select: Option<SelectedOptions>,
    cast_session: Option<CastSessionId>,
    started: bool,
    /// Closing this channel wakes up any pending Start call.
    closed_tx: async_channel::Sender<()>,
    closed_rx: async_channel::Receiver<()>,
}

#[derive(Clone)]
struct SelectedOptions {
    source_types: PickerSourceTypes,
    multiple: bool,
    cursor_mode: CursorMode,
    picker_cursor_mode: PickerCursorMode,
    persist_mode: PickerPersistMode,
    restore: Option<RestoredSelection>,
}

impl Default for SelectedOptions {
    fn default() -> Self {
        Self {
            source_types: PickerSourceTypes::MONITOR,
            multiple: false,
            cursor_mode: CursorMode::Hidden,
            picker_cursor_mode: PickerCursorMode::Hidden,
            persist_mode: PickerPersistMode::None,
            restore: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RestoredSelection {
    creation_time: i64,
    /// Monitor match strings, in stream order.
    monitors: Vec<String>,
}

#[derive(Clone)]
pub struct ScreenCastBackend(Arc<Shared>);

#[derive(Clone)]
pub struct ScreenshotBackend(Arc<Shared>);

#[derive(Clone)]
pub struct Session {
    path: OwnedObjectPath,
    shared: Arc<Shared>,
}

pub struct Request {
    on_close: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// Restore data as transported on the wire: (vendor, version, payload).
type RestoreData = (String, u32, OwnedValue);

#[derive(Debug, Default, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
struct SelectSourcesOptions {
    types: Option<u32>,
    multiple: Option<bool>,
    cursor_mode: Option<u32>,
    persist_mode: Option<u32>,
    restore_data: Option<RestoreData>,
}

#[derive(Debug, Default, SerializeDict, Type)]
#[zvariant(signature = "dict")]
struct CreateSessionResults {
    session_id: Option<String>,
}

#[derive(Debug, Default, SerializeDict, Type)]
#[zvariant(signature = "dict")]
struct EmptyResults {}

#[derive(Debug, Default, SerializeDict, Type)]
#[zvariant(signature = "dict")]
struct StartResults {
    streams: Option<Vec<(u32, StreamProperties)>>,
    persist_mode: Option<u32>,
    restore_data: Option<RestoreData>,
}

#[derive(Debug, SerializeDict, Type)]
#[zvariant(signature = "dict")]
struct StreamProperties {
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
    source_type: Option<u32>,
}

#[derive(Debug, Default, SerializeDict, Type)]
#[zvariant(signature = "dict")]
struct ScreenshotResults {
    uri: Option<String>,
}

#[derive(Debug, Default, SerializeDict, Type)]
#[zvariant(signature = "dict")]
struct PickColorResults {
    color: Option<(f64, f64, f64)>,
}

#[interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCastBackend {
    #[zbus(out_args("response", "results"))]
    async fn create_session(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(header)] header: Header<'_>,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _options: HashMap<String, OwnedValue>,
    ) -> fdo::Result<(u32, CreateSessionResults)> {
        let owner = sender_from_header(&header)?;
        debug!(%session_handle, app_id, "portal CreateSession");

        let (closed_tx, closed_rx) = async_channel::bounded(1);
        let state = SessionState {
            owner,
            select: None,
            cast_session: None,
            started: false,
            closed_tx,
            closed_rx,
        };

        let session = Session {
            path: session_handle.clone(),
            shared: self.0.clone(),
        };
        match server.at(&session_handle, session).await {
            Ok(true) => (),
            Ok(false) => {
                return Err(fdo::Error::Failed("session path already exists".to_owned()));
            }
            Err(err) => {
                return Err(fdo::Error::Failed(format!(
                    "error creating session object: {err:?}"
                )));
            }
        }
        self.0
            .sessions
            .lock()
            .unwrap()
            .insert(session_handle, state);

        let n = self.0.session_counter.fetch_add(1, Ordering::Relaxed);
        let results = CreateSessionResults {
            session_id: Some(format!("niri-{n}")),
        };
        Ok((RESPONSE_SUCCESS, results))
    }

    #[zbus(out_args("response", "results"))]
    async fn select_sources(
        &self,
        #[zbus(header)] header: Header<'_>,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        _app_id: String,
        options: SelectSourcesOptions,
    ) -> fdo::Result<(u32, EmptyResults)> {
        let owner = sender_from_header(&header)?;
        debug!(%session_handle, "portal SelectSources");

        let select = match parse_select_options(options) {
            Ok(x) => x,
            Err(err) => {
                warn!("invalid SelectSources options: {err}");
                return Ok((RESPONSE_ERROR, EmptyResults::default()));
            }
        };

        let mut sessions = self.0.sessions.lock().unwrap();
        let Some(state) = sessions.get_mut(&session_handle) else {
            return Err(fdo::Error::Failed("no such session".to_owned()));
        };
        if state.owner != owner {
            return Err(fdo::Error::AccessDenied(
                "session belongs to another D-Bus client".to_owned(),
            ));
        }
        if state.started {
            return Err(fdo::Error::Failed("session was already started".to_owned()));
        }

        state.select = Some(select);
        Ok((RESPONSE_SUCCESS, EmptyResults::default()))
    }

    #[zbus(out_args("response", "results"))]
    async fn start(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(header)] header: Header<'_>,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _parent_window: String,
        _options: HashMap<String, OwnedValue>,
    ) -> fdo::Result<(u32, StartResults)> {
        let owner = sender_from_header(&header)?;
        debug!(%session_handle, app_id, "portal Start");

        // Extract what we need from the session under the lock.
        let (select, closed_rx) = {
            let mut sessions = self.0.sessions.lock().unwrap();
            let Some(state) = sessions.get_mut(&session_handle) else {
                return Err(fdo::Error::Failed("no such session".to_owned()));
            };
            if state.owner != owner {
                return Err(fdo::Error::AccessDenied(
                    "session belongs to another D-Bus client".to_owned(),
                ));
            }
            if state.started {
                return Err(fdo::Error::Failed("session was already started".to_owned()));
            }
            state.started = true;
            (
                state.select.clone().unwrap_or_default(),
                state.closed_rx.clone(),
            )
        };

        // Try restoring a previous selection without prompting.
        let restored = select
            .restore
            .as_ref()
            .and_then(|restore| self.resolve_restored_monitors(&select, restore));

        let (sources, persist_mode, creation_time) = if let Some(sources) = restored {
            debug!("restored screen cast selection without the picker");
            let creation_time = select.restore.as_ref().unwrap().creation_time;
            (sources, select.persist_mode, Some(creation_time))
        } else {
            // Show the picker. Export a Request object so the broker can cancel it.
            let request_id = PickerRequestId {
                sender: owner.clone(),
                handle: handle.clone(),
            };
            let (reply_tx, reply_rx) = async_channel::unbounded();
            let request = PickSourcesRequest {
                id: request_id.clone(),
                options: PickerOptions {
                    app_id,
                    source_types: select.source_types,
                    multiple: select.multiple,
                    cursor_mode: select.picker_cursor_mode,
                    persist_mode: select.persist_mode,
                },
                reply: reply_tx,
            };

            let on_close = {
                let to_niri = self.0.to_niri_cast.clone();
                let id = request_id.clone();
                move || {
                    if let Err(err) = to_niri.send(ScreenCastToNiri::CancelPickSources(id)) {
                        warn!("error sending picker cancellation to niri: {err}");
                    }
                }
            };
            let request_exported = export_request(server, &handle, Box::new(on_close)).await;

            let send_res = self
                .0
                .to_niri_cast
                .send(ScreenCastToNiri::PickSources(request));
            if let Err(err) = send_res {
                if request_exported {
                    remove_request(server, &handle).await;
                }
                return Err(fdo::Error::Failed(format!(
                    "error sending picker request to niri: {err}"
                )));
            }

            let reply = match select_or_closed(reply_rx.recv(), &closed_rx).await {
                Some(Ok(reply)) => Some(reply),
                Some(Err(_)) => None,
                // Session was closed while the picker was open.
                None => {
                    let to_niri = self.0.to_niri_cast.clone();
                    let _ = to_niri.send(ScreenCastToNiri::CancelPickSources(request_id));
                    None
                }
            };

            if request_exported {
                remove_request(server, &handle).await;
            }

            let selection = match reply {
                Some(PickSourcesReply::Selected(selection)) => selection,
                Some(PickSourcesReply::Cancelled) | None => {
                    return Ok((RESPONSE_CANCELLED, StartResults::default()));
                }
                Some(PickSourcesReply::Failed(message)) => {
                    warn!("screen cast picker failed: {message}");
                    return Ok((RESPONSE_ERROR, StartResults::default()));
                }
            };

            let options = PickerOptions {
                app_id: String::new(),
                source_types: select.source_types,
                multiple: select.multiple,
                cursor_mode: select.picker_cursor_mode,
                persist_mode: select.persist_mode,
            };
            match validate_selection(&options, selection) {
                Ok((sources, persist_mode)) => (sources, persist_mode, None),
                Err(err) => {
                    warn!("screen cast picker returned an invalid selection: {err}");
                    return Ok((RESPONSE_ERROR, StartResults::default()));
                }
            }
        };

        // Start the casts and collect PipeWire node ids.
        let cast_session = CastSessionId::next();
        self.0
            .cast_paths
            .lock()
            .unwrap()
            .insert(cast_session, session_handle.clone());
        {
            let mut sessions = self.0.sessions.lock().unwrap();
            match sessions.get_mut(&session_handle) {
                Some(state) => state.cast_session = Some(cast_session),
                // The session was closed while the picker was open.
                None => {
                    self.0.cast_paths.lock().unwrap().remove(&cast_session);
                    return Ok((RESPONSE_CANCELLED, StartResults::default()));
                }
            }
        }

        let mut streams = Vec::with_capacity(sources.len());
        for target in &sources {
            let (node_tx, node_rx) = async_channel::bounded(1);
            let msg = ScreenCastToNiri::StartCast {
                session_id: cast_session,
                stream_id: CastStreamId::next(),
                target: target.clone(),
                cursor_mode: select.cursor_mode,
                node_sink: NodeIdSink::Channel(node_tx),
            };
            if let Err(err) = self.0.to_niri_cast.send(msg) {
                warn!("error sending StartCast to niri: {err}");
                self.fail_start(server, &session_handle, cast_session).await;
                return Ok((RESPONSE_ERROR, StartResults::default()));
            }

            let node_id = match select_or_closed(node_rx.recv(), &closed_rx).await {
                Some(Ok(node_id)) => node_id,
                // Cast failed to start (niri called stop_cast) or session closed.
                Some(Err(_)) | None => {
                    self.fail_start(server, &session_handle, cast_session).await;
                    return Ok((RESPONSE_ERROR, StartResults::default()));
                }
            };

            streams.push((node_id, self.stream_properties(target)));
        }

        let restore_data = if persist_mode > PickerPersistMode::None {
            self.serialize_restore_data(&sources, creation_time)
        } else {
            None
        };

        let results = StartResults {
            streams: Some(streams),
            persist_mode: Some(persist_mode as u32),
            restore_data,
        };
        Ok((RESPONSE_SUCCESS, results))
    }

    #[zbus(property, name = "AvailableSourceTypes")]
    async fn available_source_types(&self) -> u32 {
        SOURCE_TYPE_MONITOR | SOURCE_TYPE_WINDOW
    }

    #[zbus(property, name = "AvailableCursorModes")]
    async fn available_cursor_modes(&self) -> u32 {
        PickerCursorMode::Hidden as u32
            | PickerCursorMode::Embedded as u32
            | PickerCursorMode::Metadata as u32
    }

    #[zbus(property, name = "version")]
    async fn version(&self) -> u32 {
        4
    }
}

#[interface(name = "org.freedesktop.impl.portal.Screenshot")]
impl ScreenshotBackend {
    #[zbus(out_args("response", "results"))]
    async fn screenshot(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        _parent_window: String,
        _options: HashMap<String, OwnedValue>,
    ) -> fdo::Result<(u32, ScreenshotResults)> {
        debug!(app_id, "portal Screenshot");

        let on_close = {
            let to_niri = self.0.to_niri_screenshot.clone();
            move || {
                let _ = to_niri.send(ScreenshotToNiri::CancelScreenshot);
            }
        };
        let request_exported = export_request(server, &handle, Box::new(on_close)).await;

        let (reply_tx, reply_rx) = async_channel::bounded(1);
        let send_res = self
            .0
            .to_niri_screenshot
            .send(ScreenshotToNiri::InteractiveScreenshot(reply_tx));

        let result = match send_res {
            Ok(()) => reply_rx.recv().await,
            Err(err) => {
                if request_exported {
                    remove_request(server, &handle).await;
                }
                return Err(fdo::Error::Failed(format!(
                    "error sending screenshot request to niri: {err}"
                )));
            }
        };

        if request_exported {
            remove_request(server, &handle).await;
        }

        match result {
            Ok(Ok(path)) => {
                let uri = file_uri(&path)?;
                let results = ScreenshotResults { uri: Some(uri) };
                Ok((RESPONSE_SUCCESS, results))
            }
            Ok(Err(ScreenshotPortalError::Cancelled)) => {
                Ok((RESPONSE_CANCELLED, ScreenshotResults::default()))
            }
            Ok(Err(ScreenshotPortalError::Failed(message))) => {
                warn!("screenshot failed: {message}");
                Ok((RESPONSE_ERROR, ScreenshotResults::default()))
            }
            Err(_) => Ok((RESPONSE_ERROR, ScreenshotResults::default())),
        }
    }

    #[zbus(out_args("response", "results"))]
    async fn pick_color(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        _parent_window: String,
        _options: HashMap<String, OwnedValue>,
    ) -> fdo::Result<(u32, PickColorResults)> {
        debug!(app_id, "portal PickColor");

        let on_close = {
            let to_niri = self.0.to_niri_screenshot.clone();
            move || {
                let _ = to_niri.send(ScreenshotToNiri::CancelScreenshot);
            }
        };
        let request_exported = export_request(server, &handle, Box::new(on_close)).await;

        let (reply_tx, reply_rx) = async_channel::bounded(1);
        let send_res = self
            .0
            .to_niri_screenshot
            .send(ScreenshotToNiri::PickColor(reply_tx));

        let result = match send_res {
            Ok(()) => reply_rx.recv().await,
            Err(err) => {
                if request_exported {
                    remove_request(server, &handle).await;
                }
                return Err(fdo::Error::Failed(format!(
                    "error sending pick color request to niri: {err}"
                )));
            }
        };

        if request_exported {
            remove_request(server, &handle).await;
        }

        match result {
            Ok(Some(color)) => {
                let [r, g, b] = color.rgb;
                let results = PickColorResults {
                    color: Some((r, g, b)),
                };
                Ok((RESPONSE_SUCCESS, results))
            }
            Ok(None) => Ok((RESPONSE_CANCELLED, PickColorResults::default())),
            Err(_) => Ok((RESPONSE_ERROR, PickColorResults::default())),
        }
    }

    #[zbus(property, name = "version")]
    async fn version(&self) -> u32 {
        2
    }
}

#[interface(name = "org.freedesktop.impl.portal.Session")]
impl Session {
    async fn close(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        debug!(path = %self.path, "portal Session Close");
        self.do_close(server, &ctxt, true).await;
        Ok(())
    }

    #[zbus(signal)]
    async fn closed(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    async fn version(&self) -> u32 {
        1
    }
}

#[interface(name = "org.freedesktop.impl.portal.Request")]
impl Request {
    async fn close(&self) -> fdo::Result<()> {
        if let Some(on_close) = self.on_close.lock().unwrap().take() {
            on_close();
        }
        Ok(())
    }
}

impl Session {
    pub async fn close_from_compositor(&self, server: &ObjectServer, ctxt: &SignalEmitter<'_>) {
        self.do_close(server, ctxt, false).await;
    }

    async fn do_close(&self, server: &ObjectServer, ctxt: &SignalEmitter<'_>, send_stop: bool) {
        let state = self.shared.sessions.lock().unwrap().remove(&self.path);
        let Some(state) = state else {
            return;
        };

        // Wake up a pending Start call.
        state.closed_tx.close();

        if let Some(cast_session) = state.cast_session {
            self.shared.cast_paths.lock().unwrap().remove(&cast_session);
            if send_stop {
                let msg = ScreenCastToNiri::StopCast {
                    session_id: cast_session,
                };
                if let Err(err) = self.shared.to_niri_cast.send(msg) {
                    warn!("error sending StopCast to niri: {err}");
                }
            }
        }

        if let Err(err) = Session::closed(ctxt).await {
            warn!("error emitting portal session Closed signal: {err:?}");
        }
        if let Err(err) = server.remove::<Session, _>(&self.path).await {
            warn!("error removing portal session object: {err:?}");
        }
    }
}

impl ScreenCastBackend {
    /// Resolves restored monitor match strings against current outputs.
    ///
    /// Returns None when the restore data cannot be applied, in which case the
    /// picker is shown normally.
    fn resolve_restored_monitors(
        &self,
        select: &SelectedOptions,
        restore: &RestoredSelection,
    ) -> Option<Vec<StreamTargetId>> {
        if !select.source_types.contains(PickerSourceTypes::MONITOR) {
            return None;
        }
        if restore.monitors.is_empty() {
            return None;
        }
        if !select.multiple && restore.monitors.len() != 1 {
            return None;
        }

        let ipc_outputs = self.0.ipc_outputs.lock().unwrap();
        let mut sources = Vec::with_capacity(restore.monitors.len());
        for match_string in &restore.monitors {
            let output = ipc_outputs
                .values()
                .find(|o| o.logical.is_some() && monitor_match_string(o) == *match_string)?;
            sources.push(StreamTargetId::Output {
                name: output.name.clone(),
            });
        }
        Some(sources)
    }

    fn stream_properties(&self, target: &StreamTargetId) -> StreamProperties {
        match target {
            StreamTargetId::Output { name } => {
                let ipc_outputs = self.0.ipc_outputs.lock().unwrap();
                let logical = ipc_outputs
                    .values()
                    .find(|o| o.name == *name)
                    .and_then(|o| o.logical.as_ref());
                StreamProperties {
                    position: logical.map(|l| (l.x, l.y)),
                    size: logical.map(|l| (l.width as i32, l.height as i32)),
                    source_type: Some(SOURCE_TYPE_MONITOR),
                }
            }
            StreamTargetId::Window { .. } => StreamProperties {
                position: None,
                size: None,
                source_type: Some(SOURCE_TYPE_WINDOW),
            },
        }
    }

    /// Serializes restore data for a selection.
    ///
    /// Only all-monitor selections are serialized; window identity is not
    /// stable across sessions, so selections containing windows produce no
    /// restore data and the user will pick again next time.
    fn serialize_restore_data(
        &self,
        sources: &[StreamTargetId],
        creation_time: Option<i64>,
    ) -> Option<RestoreData> {
        let ipc_outputs = self.0.ipc_outputs.lock().unwrap();
        let mut monitors = Vec::with_capacity(sources.len());
        for source in sources {
            match source {
                StreamTargetId::Output { name } => {
                    let output = ipc_outputs.values().find(|o| o.name == *name)?;
                    monitors.push(monitor_match_string(output));
                }
                StreamTargetId::Window { .. } => return None,
            }
        }
        drop(ipc_outputs);

        let now = real_time_micros();
        let creation_time = creation_time.unwrap_or(now);
        Some(build_restore_data(creation_time, now, &monitors))
    }

    async fn fail_start(
        &self,
        server: &ObjectServer,
        session_handle: &OwnedObjectPath,
        cast_session: CastSessionId,
    ) {
        self.0.cast_paths.lock().unwrap().remove(&cast_session);
        let _ = self.0.to_niri_cast.send(ScreenCastToNiri::StopCast {
            session_id: cast_session,
        });

        // Close the session: a failed Start invalidates it.
        if let Ok(iface) = server
            .interface::<_, Session>(session_handle.as_ref())
            .await
        {
            let ctxt = iface.signal_emitter().clone();
            iface.get().await.do_close(server, &ctxt, false).await;
        }
    }
}

fn parse_select_options(options: SelectSourcesOptions) -> Result<SelectedOptions, String> {
    let types = options.types.unwrap_or(SOURCE_TYPE_MONITOR);
    let source_types = PickerSourceTypes::from_bits(types)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| format!("invalid source types {types}"))?;

    let cursor_mode = options.cursor_mode.unwrap_or(PickerCursorMode::Hidden as u32);
    let picker_cursor_mode = PickerCursorMode::try_from(cursor_mode).map_err(str::to_owned)?;
    let cursor_mode = match picker_cursor_mode {
        PickerCursorMode::Hidden => CursorMode::Hidden,
        PickerCursorMode::Embedded => CursorMode::Embedded,
        PickerCursorMode::Metadata => CursorMode::Metadata,
    };

    let persist_mode = options.persist_mode.unwrap_or(0);
    let persist_mode = PickerPersistMode::try_from(persist_mode).map_err(str::to_owned)?;

    let restore = options
        .restore_data
        .as_ref()
        .and_then(|(vendor, version, inner)| parse_restore_data(vendor, *version, inner));

    Ok(SelectedOptions {
        source_types,
        multiple: options.multiple.unwrap_or(false),
        cursor_mode,
        picker_cursor_mode,
        persist_mode,
        restore,
    })
}

/// Parses restore data with the SHORINNIRI/GNOME v1 inner format
/// `(xxa(uuv))`.
///
/// Returns None (fall back to the picker) for unknown vendors or versions,
/// malformed data, and selections containing non-monitor sources.
fn parse_restore_data(vendor: &str, version: u32, inner: &Value<'_>) -> Option<RestoredSelection> {
    if !RESTORE_VENDORS.contains(&vendor) || version != RESTORE_FORMAT_VERSION {
        return None;
    }

    let inner = unwrap_variant(inner);
    let inner = inner.downcast_ref::<zbus::zvariant::Structure>().ok()?;
    let inner_fields = inner.fields();
    if inner_fields.len() != 3 {
        return None;
    }

    let creation_time: i64 = inner_fields[0].downcast_ref().ok()?;
    let _last_used_time: i64 = inner_fields[1].downcast_ref().ok()?;
    let streams = inner_fields[2].downcast_ref::<zbus::zvariant::Array>().ok()?;

    let mut monitors = Vec::new();
    for stream in streams.iter() {
        let stream = unwrap_variant(&stream);
        let stream = stream.downcast_ref::<zbus::zvariant::Structure>().ok()?;
        let stream_fields = stream.fields();
        if stream_fields.len() != 3 {
            return None;
        }

        let _id: u32 = stream_fields[0].downcast_ref().ok()?;
        let source_type: u32 = stream_fields[1].downcast_ref().ok()?;
        if source_type != SOURCE_TYPE_MONITOR {
            // Window and virtual sources are not restored.
            return None;
        }

        let data = unwrap_variant(&stream_fields[2]);
        let match_string: &str = data.downcast_ref().ok()?;
        monitors.push(match_string.to_owned());
    }

    if monitors.is_empty() {
        return None;
    }

    Some(RestoredSelection {
        creation_time,
        monitors,
    })
}

fn unwrap_variant<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
    match value {
        Value::Value(inner) => inner,
        other => other,
    }
}

fn build_restore_data(creation_time: i64, last_used_time: i64, monitors: &[String]) -> RestoreData {
    let streams: Vec<(u32, u32, Value)> = monitors
        .iter()
        .enumerate()
        .map(|(i, match_string)| {
            (
                i as u32,
                SOURCE_TYPE_MONITOR,
                Value::new(Value::from(match_string.clone())),
            )
        })
        .collect();

    let inner = Value::from((creation_time, last_used_time, streams));
    (
        RESTORE_VENDOR.to_owned(),
        RESTORE_FORMAT_VERSION,
        inner.try_to_owned().unwrap(),
    )
}

/// Constructs the same monitor identity string that
/// xdg-desktop-portal-shorinniri persisted, so existing restore tokens keep
/// working.
fn monitor_match_string(output: &niri_ipc::Output) -> String {
    let serial = output.serial.as_deref().unwrap_or(&output.name);
    if output.make == "unknown" && output.model == "unknown" && serial == "unknown" {
        output.name.clone()
    } else {
        format!("{}:{}:{}", output.make, output.model, serial)
    }
}

fn real_time_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

/// Awaits `future`, aborting early when the session's closed channel fires.
///
/// Returns None when the session was closed first.
async fn select_or_closed<T>(
    future: impl std::future::Future<Output = T>,
    closed_rx: &async_channel::Receiver<()>,
) -> Option<T> {
    let future = std::pin::pin!(future);
    let closed = std::pin::pin!(closed_rx.recv());
    match select(future, closed).await {
        Either::Left((value, _)) => Some(value),
        Either::Right(_) => None,
    }
}

async fn export_request(
    server: &ObjectServer,
    path: &OwnedObjectPath,
    on_close: Box<dyn FnOnce() + Send>,
) -> bool {
    let request = Request {
        on_close: Mutex::new(Some(on_close)),
    };
    match server.at(path, request).await {
        Ok(true) => true,
        Ok(false) => {
            warn!(%path, "portal request path already exists");
            false
        }
        Err(err) => {
            warn!(%path, "error exporting portal request object: {err:?}");
            false
        }
    }
}

async fn remove_request(server: &ObjectServer, path: &OwnedObjectPath) {
    if let Err(err) = server.remove::<Request, _>(path).await {
        warn!(%path, "error removing portal request object: {err:?}");
    }
}

fn sender_from_header(header: &Header<'_>) -> fdo::Result<OwnedUniqueName> {
    header
        .sender()
        .map(|sender| OwnedUniqueName::from(sender.to_owned()))
        .ok_or_else(|| fdo::Error::Failed("D-Bus call has no sender".to_owned()))
}

/// Starts the portal backend and returns its connection along with the cast
/// session registry used by [`crate::niri::Niri::stop_cast`].
pub fn start(
    to_niri_cast: calloop::channel::Sender<ScreenCastToNiri>,
    to_niri_screenshot: calloop::channel::Sender<ScreenshotToNiri>,
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
) -> anyhow::Result<(zbus::blocking::Connection, PortalCastMap)> {
    let cast_paths = PortalCastMap::default();
    let shared = Arc::new(Shared {
        to_niri_cast,
        to_niri_screenshot,
        ipc_outputs,
        sessions: Mutex::new(HashMap::new()),
        cast_paths: cast_paths.clone(),
        session_counter: AtomicU64::new(0),
    });

    let conn = zbus::blocking::Connection::session()?;
    let flags = RequestNameFlags::AllowReplacement
        | RequestNameFlags::ReplaceExisting
        | RequestNameFlags::DoNotQueue;

    conn.object_server()
        .at(PORTAL_PATH, ScreenCastBackend(shared.clone()))?;
    conn.object_server()
        .at(PORTAL_PATH, ScreenshotBackend(shared.clone()))?;
    conn.request_name_with_flags(BUS_NAME, flags)?;

    let async_conn = conn.inner().clone();
    let future = async move {
        if let Err(err) = monitor_disappeared_clients(&async_conn, shared).await {
            warn!("error monitoring portal clients: {err:?}");
        }
    };
    let task = conn
        .inner()
        .executor()
        .spawn(future, "monitor disappearing portal clients");
    task.detach();

    Ok((conn, cast_paths))
}

/// Closes sessions whose owning D-Bus client (normally the broker) went away.
async fn monitor_disappeared_clients(
    conn: &zbus::Connection,
    shared: Arc<Shared>,
) -> anyhow::Result<()> {
    let proxy = fdo::DBusProxy::new(conn)
        .await
        .context("error creating a DBusProxy")?;
    let mut stream = proxy
        .receive_name_owner_changed_with_args(&[(2, UniqueName::null_value())])
        .await
        .context("error creating a NameOwnerChanged stream")?;

    while let Some(signal) = stream.next().await {
        let args = signal
            .args()
            .context("error retrieving NameOwnerChanged args")?;
        let Some(name) = &**args.old_owner() else {
            continue;
        };
        if args.new_owner().is_some() {
            continue;
        }

        let owner = OwnedUniqueName::from(name.to_owned());
        let paths: Vec<OwnedObjectPath> = shared
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, state)| state.owner == owner)
            .map(|(path, _)| path.clone())
            .collect();

        let server = conn.object_server();
        for path in paths {
            if let Ok(iface) = server.interface::<_, Session>(&path).await {
                let ctxt = iface.signal_emitter().clone();
                iface.get().await.do_close(server, &ctxt, true).await;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::Type as _;

    use super::*;

    fn output(name: &str, make: &str, model: &str, serial: Option<&str>) -> niri_ipc::Output {
        niri_ipc::Output {
            name: name.to_owned(),
            make: make.to_owned(),
            model: model.to_owned(),
            serial: serial.map(str::to_owned),
            physical_size: None,
            modes: vec![],
            current_mode: None,
            is_custom_mode: false,
            vrr_supported: false,
            vrr_enabled: false,
            max_bpc: None,
            logical: None,
        }
    }

    #[test]
    fn match_string_follows_shorinniri_rules() {
        assert_eq!(
            monitor_match_string(&output("DP-1", "Dell", "U2720Q", Some("ABC123"))),
            "Dell:U2720Q:ABC123"
        );
        // Serial falls back to the connector, mirroring niri's DisplayConfig.
        assert_eq!(
            monitor_match_string(&output("DP-1", "Dell", "U2720Q", None)),
            "Dell:U2720Q:DP-1"
        );
        assert_eq!(
            monitor_match_string(&output("Virtual-1", "unknown", "unknown", Some("unknown"))),
            "Virtual-1"
        );
    }

    #[test]
    fn restore_data_roundtrips() {
        let monitors = vec!["Dell:U2720Q:ABC123".to_owned(), "DP-2".to_owned()];
        let (vendor, version, inner) = build_restore_data(123, 456, &monitors);

        assert_eq!(vendor, RESTORE_VENDOR);
        assert_eq!(inner.value_signature(), "(xxa(uuv))");

        let inner = Value::from(inner);
        let parsed = parse_restore_data(&vendor, version, &inner).unwrap();
        assert_eq!(
            parsed,
            RestoredSelection {
                creation_time: 123,
                monitors,
            }
        );
    }

    #[test]
    fn restore_data_rejects_unknown_vendors_and_windows() {
        let inner = Value::from((
            1i64,
            2i64,
            vec![(
                0u32,
                SOURCE_TYPE_MONITOR,
                Value::new(Value::from("DP-1")),
            )],
        ));
        assert_eq!(parse_restore_data("KDE", RESTORE_FORMAT_VERSION, &inner), None);
        assert_eq!(parse_restore_data(RESTORE_VENDOR, 2, &inner), None);
        assert!(parse_restore_data("GNOME", RESTORE_FORMAT_VERSION, &inner).is_some());
        assert!(parse_restore_data(RESTORE_VENDOR, RESTORE_FORMAT_VERSION, &inner).is_some());

        let window_inner = Value::from((
            1i64,
            2i64,
            vec![(
                0u32,
                SOURCE_TYPE_WINDOW,
                Value::new(Value::from(("app", "title"))),
            )],
        ));
        assert_eq!(
            parse_restore_data(RESTORE_VENDOR, RESTORE_FORMAT_VERSION, &window_inner),
            None
        );
    }

    #[test]
    fn result_dict_signatures_match_the_protocol() {
        assert_eq!(CreateSessionResults::SIGNATURE.to_string(), "a{sv}");
        assert_eq!(StartResults::SIGNATURE.to_string(), "a{sv}");
        assert_eq!(SelectSourcesOptions::SIGNATURE.to_string(), "a{sv}");
        assert_eq!(
            Vec::<(u32, StreamProperties)>::SIGNATURE.to_string(),
            "a(ua{sv})"
        );
    }

    #[test]
    fn select_options_parse_and_validate() {
        let defaults = parse_select_options(SelectSourcesOptions::default()).unwrap();
        assert_eq!(defaults.source_types, PickerSourceTypes::MONITOR);
        assert!(!defaults.multiple);
        assert_eq!(defaults.cursor_mode, CursorMode::Hidden);
        assert_eq!(defaults.persist_mode, PickerPersistMode::None);
        assert!(defaults.restore.is_none());

        let options = SelectSourcesOptions {
            types: Some(3),
            multiple: Some(true),
            cursor_mode: Some(PickerCursorMode::Metadata as u32),
            persist_mode: Some(2),
            restore_data: None,
        };
        let parsed = parse_select_options(options).unwrap();
        assert_eq!(
            parsed.source_types,
            PickerSourceTypes::MONITOR | PickerSourceTypes::WINDOW
        );
        assert!(parsed.multiple);
        assert_eq!(parsed.cursor_mode, CursorMode::Metadata);
        assert_eq!(parsed.persist_mode, PickerPersistMode::Persistent);

        let bad = SelectSourcesOptions {
            types: Some(8),
            ..Default::default()
        };
        assert!(parse_select_options(bad).is_err());

        let bad = SelectSourcesOptions {
            cursor_mode: Some(3),
            ..Default::default()
        };
        assert!(parse_select_options(bad).is_err());
    }
}
