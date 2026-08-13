use std::collections::{HashMap, HashSet};
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use anyhow::Context as _;
use futures_util::StreamExt;
use serde::Deserialize;
use zbus::fdo::RequestNameFlags;
use zbus::message::Header;
use zbus::names::{OwnedUniqueName, UniqueName};
use zbus::object_server::{InterfaceRef, SignalEmitter};
use zbus::zvariant::{DeserializeDict, NoneValue, OwnedObjectPath, SerializeDict, Type, Value};
use zbus::{fdo, interface, ObjectServer};

use super::niri_portal_screen_cast::{
    ConsumeTokenError, PickSourcesRequest, PickerRequestId, PortalScreenCast, SelectionTokens,
};
use super::Start;
use crate::backend::IpcOutputMap;
use crate::utils::{CastSessionId, CastStreamId};

type SessionRegistry = Arc<Mutex<HashMap<CastSessionId, (Session, InterfaceRef<Session>)>>>;

#[derive(Clone)]
pub struct ScreenCast {
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    to_niri: calloop::channel::Sender<ScreenCastToNiri>,
    sessions: SessionRegistry,
    portal: PortalScreenCast,
    tokens: SelectionTokens,
}

#[derive(Clone)]
pub struct Session {
    id: CastSessionId,
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    to_niri: calloop::channel::Sender<ScreenCastToNiri>,
    #[allow(clippy::type_complexity)]
    streams: Arc<Mutex<Vec<(Stream, InterfaceRef<Stream>)>>>,
    stopped: Arc<AtomicBool>,
    owner: OwnedUniqueName,
    authorized_targets: Option<Arc<HashSet<StreamTargetId>>>,
    sessions: Weak<Mutex<HashMap<CastSessionId, (Session, InterfaceRef<Session>)>>>,
}

#[derive(Debug, Default, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
struct CreateSessionProperties {
    #[zvariant(rename = "remote-desktop-session-id")]
    remote_desktop_session_id: Option<String>,
    #[zvariant(rename = "selection-token")]
    selection_token: Option<Vec<u8>>,
}

#[derive(Debug, Default, Deserialize, Type, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    #[default]
    Hidden = 0,
    Embedded = 1,
    Metadata = 2,
}

#[derive(Debug, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
struct RecordMonitorProperties {
    #[zvariant(rename = "cursor-mode")]
    cursor_mode: Option<CursorMode>,
    #[zvariant(rename = "is-recording")]
    _is_recording: Option<bool>,
}

#[derive(Debug, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
struct RecordWindowProperties {
    #[zvariant(rename = "window-id")]
    window_id: u64,
    #[zvariant(rename = "cursor-mode")]
    cursor_mode: Option<CursorMode>,
    #[zvariant(rename = "is-recording")]
    _is_recording: Option<bool>,
}

#[derive(Clone)]
pub struct Stream {
    id: CastStreamId,
    session_id: CastSessionId,
    target: StreamTarget,
    cursor_mode: CursorMode,
    was_started: Arc<AtomicBool>,
    to_niri: calloop::channel::Sender<ScreenCastToNiri>,
}

#[derive(Clone)]
enum StreamTarget {
    // FIXME: update on scale changes and whatnot.
    Output(niri_ipc::Output),
    Window { id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StreamTargetId {
    Output { name: String },
    Window { id: u64 },
}

#[derive(Debug, SerializeDict, Type, Value)]
#[zvariant(signature = "dict")]
struct StreamParameters {
    /// Position of the stream in logical coordinates.
    position: Option<(i32, i32)>,
    /// Size of the stream in logical coordinates.
    size: Option<(i32, i32)>,
}

pub enum ScreenCastToNiri {
    PickSources(PickSourcesRequest),
    CancelPickSources(PickerRequestId),
    StartCast {
        session_id: CastSessionId,
        stream_id: CastStreamId,
        target: StreamTargetId,
        cursor_mode: CursorMode,
        node_sink: NodeIdSink,
    },
    StopCast {
        session_id: CastSessionId,
    },
}

/// Where to deliver the PipeWire node id once the stream is created.
#[derive(Clone)]
pub enum NodeIdSink {
    /// Emit the org.gnome.Mutter.ScreenCast.Stream.PipeWireStreamAdded signal.
    MutterSignal(SignalEmitter<'static>),
    /// Send the node id over a channel (used by the built-in portal backend).
    Channel(async_channel::Sender<u32>),
}

impl NodeIdSink {
    pub fn deliver(&self, node_id: u32) -> anyhow::Result<()> {
        match self {
            NodeIdSink::MutterSignal(ctxt) => async_io::block_on(async {
                Stream::pipe_wire_stream_added(ctxt, node_id)
                    .await
                    .context("error sending PipeWireStreamAdded")
            }),
            NodeIdSink::Channel(tx) => tx
                .try_send(node_id)
                .context("error sending node id to the portal backend"),
        }
    }
}

#[interface(name = "org.gnome.Mutter.ScreenCast")]
impl ScreenCast {
    async fn create_session(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(header)] header: Header<'_>,
        properties: CreateSessionProperties,
    ) -> fdo::Result<OwnedObjectPath> {
        if properties.remote_desktop_session_id.is_some() {
            return Err(fdo::Error::Failed(
                "there are no remote desktop sessions".to_owned(),
            ));
        }

        let owner = sender_from_header(&header)?;
        let authorized_targets = match properties.selection_token {
            Some(token) => {
                let targets = self
                    .tokens
                    .consume(&owner, &token)
                    .map_err(|err| match err {
                        ConsumeTokenError::InvalidLength => fdo::Error::InvalidArgs(
                            "selection-token has an invalid length".to_owned(),
                        ),
                        ConsumeTokenError::Invalid => {
                            fdo::Error::AccessDenied("invalid selection-token".to_owned())
                        }
                    })?;
                Some(Arc::new(targets))
            }
            None => None,
        };

        let session_id = CastSessionId::next();
        let path = format!("/org/gnome/Mutter/ScreenCast/Session/u{}", session_id.get());
        let path = OwnedObjectPath::try_from(path).unwrap();

        let session = Session::new(
            session_id,
            self.ipc_outputs.clone(),
            self.to_niri.clone(),
            owner,
            authorized_targets,
            Arc::downgrade(&self.sessions),
        );
        match server.at(&path, session.clone()).await {
            Ok(true) => {
                let iface = server.interface(&path).await.unwrap();
                self.sessions
                    .lock()
                    .unwrap()
                    .insert(session_id, (session, iface));
            }
            Ok(false) => return Err(fdo::Error::Failed("session path already exists".to_owned())),
            Err(err) => {
                return Err(fdo::Error::Failed(format!(
                    "error creating session object: {err:?}"
                )))
            }
        }

        Ok(path)
    }

    #[zbus(property)]
    async fn version(&self) -> i32 {
        4
    }
}

#[interface(name = "org.gnome.Mutter.ScreenCast.Session")]
impl Session {
    async fn start(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        debug!("start");
        self.ensure_owner(&header)?;
        self.ensure_open()?;

        for (stream, iface) in &*self.streams.lock().unwrap() {
            stream.start(iface.signal_emitter().clone());
        }

        Ok(())
    }

    async fn stop(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        debug!("stop");
        self.ensure_owner(&header)?;
        self.stop_internal(server, &ctxt).await;
        Ok(())
    }

    async fn record_monitor(
        &mut self,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(header)] header: Header<'_>,
        connector: &str,
        properties: RecordMonitorProperties,
    ) -> fdo::Result<OwnedObjectPath> {
        debug!(connector, ?properties, "record_monitor");
        self.ensure_owner(&header)?;
        self.ensure_open()?;
        self.authorize_target(&StreamTargetId::Output {
            name: connector.to_owned(),
        })?;

        let output = {
            let ipc_outputs = self.ipc_outputs.lock().unwrap();
            ipc_outputs.values().find(|o| o.name == connector).cloned()
        };
        let Some(output) = output else {
            return Err(fdo::Error::Failed("no such monitor".to_owned()));
        };

        if output.logical.is_none() {
            return Err(fdo::Error::Failed("monitor is disabled".to_owned()));
        }

        let stream_id = CastStreamId::next();
        let path = format!("/org/gnome/Mutter/ScreenCast/Stream/u{}", stream_id.get());
        let path = OwnedObjectPath::try_from(path).unwrap();

        let cursor_mode = properties.cursor_mode.unwrap_or_default();

        let target = StreamTarget::Output(output);
        let stream = Stream::new(
            stream_id,
            self.id,
            target,
            cursor_mode,
            self.to_niri.clone(),
        );
        match server.at(&path, stream.clone()).await {
            Ok(true) => {
                let iface = server.interface(&path).await.unwrap();
                self.streams.lock().unwrap().push((stream, iface));
            }
            Ok(false) => return Err(fdo::Error::Failed("stream path already exists".to_owned())),
            Err(err) => {
                return Err(fdo::Error::Failed(format!(
                    "error creating stream object: {err:?}"
                )))
            }
        }

        Ok(path)
    }

    async fn record_window(
        &mut self,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(header)] header: Header<'_>,
        properties: RecordWindowProperties,
    ) -> fdo::Result<OwnedObjectPath> {
        debug!(?properties, "record_window");
        self.ensure_owner(&header)?;
        self.ensure_open()?;
        self.authorize_target(&StreamTargetId::Window {
            id: properties.window_id,
        })?;

        let stream_id = CastStreamId::next();
        let path = format!("/org/gnome/Mutter/ScreenCast/Stream/u{}", stream_id.get());
        let path = OwnedObjectPath::try_from(path).unwrap();

        let cursor_mode = properties.cursor_mode.unwrap_or_default();

        let target = StreamTarget::Window {
            id: properties.window_id,
        };
        let stream = Stream::new(
            stream_id,
            self.id,
            target,
            cursor_mode,
            self.to_niri.clone(),
        );
        match server.at(&path, stream.clone()).await {
            Ok(true) => {
                let iface = server.interface(&path).await.unwrap();
                self.streams.lock().unwrap().push((stream, iface));
            }
            Ok(false) => return Err(fdo::Error::Failed("stream path already exists".to_owned())),
            Err(err) => {
                return Err(fdo::Error::Failed(format!(
                    "error creating stream object: {err:?}"
                )))
            }
        }

        Ok(path)
    }

    #[zbus(signal)]
    async fn closed(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;
}

#[interface(name = "org.gnome.Mutter.ScreenCast.Stream")]
impl Stream {
    #[zbus(signal)]
    pub async fn pipe_wire_stream_added(ctxt: &SignalEmitter<'_>, node_id: u32)
        -> zbus::Result<()>;

    #[zbus(property)]
    async fn parameters(&self) -> StreamParameters {
        match &self.target {
            StreamTarget::Output(output) => {
                let logical = output.logical.as_ref().unwrap();
                StreamParameters {
                    position: Some((logical.x, logical.y)),
                    size: Some((logical.width as i32, logical.height as i32)),
                }
            }
            StreamTarget::Window { .. } => StreamParameters {
                position: None,
                size: None,
            },
        }
    }
}

impl ScreenCast {
    pub fn new(
        ipc_outputs: Arc<Mutex<IpcOutputMap>>,
        to_niri: calloop::channel::Sender<ScreenCastToNiri>,
    ) -> Self {
        let tokens = SelectionTokens::default();
        Self {
            ipc_outputs,
            portal: PortalScreenCast::new(to_niri.clone(), tokens.clone()),
            tokens,
            to_niri,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Start for ScreenCast {
    fn start(self) -> anyhow::Result<zbus::blocking::Connection> {
        let portal = self.portal.clone();
        let sessions = self.sessions.clone();
        let conn = zbus::blocking::Connection::session()?;
        let flags = RequestNameFlags::AllowReplacement
            | RequestNameFlags::ReplaceExisting
            | RequestNameFlags::DoNotQueue;

        conn.object_server()
            .at("/org/gnome/Mutter/ScreenCast", self)?;
        conn.object_server()
            .at("/org/niri/Portal/ScreenCast", portal.clone())?;
        conn.request_name_with_flags("org.gnome.Mutter.ScreenCast", flags)?;

        let async_conn = conn.inner().clone();
        let future = async move {
            if let Err(err) = monitor_disappeared_clients(&async_conn, sessions, portal).await {
                warn!("error monitoring screen cast clients: {err:?}");
            }
        };
        let task = conn
            .inner()
            .executor()
            .spawn(future, "monitor disappearing screen cast clients");
        task.detach();

        Ok(conn)
    }
}

impl Session {
    pub fn new(
        id: CastSessionId,
        ipc_outputs: Arc<Mutex<IpcOutputMap>>,
        to_niri: calloop::channel::Sender<ScreenCastToNiri>,
        owner: OwnedUniqueName,
        authorized_targets: Option<Arc<HashSet<StreamTargetId>>>,
        sessions: Weak<Mutex<HashMap<CastSessionId, (Session, InterfaceRef<Session>)>>>,
    ) -> Self {
        Self {
            id,
            ipc_outputs,
            streams: Arc::new(Mutex::new(vec![])),
            to_niri,
            stopped: Arc::new(AtomicBool::new(false)),
            owner,
            authorized_targets,
            sessions,
        }
    }

    pub async fn stop_from_compositor(&self, server: &ObjectServer, ctxt: SignalEmitter<'_>) {
        self.stop_internal(server, &ctxt).await;
    }

    fn ensure_owner(&self, header: &Header<'_>) -> fdo::Result<()> {
        if sender_from_header(header)? != self.owner {
            return Err(fdo::Error::AccessDenied(
                "screen cast session belongs to another D-Bus client".to_owned(),
            ));
        }
        Ok(())
    }

    fn ensure_open(&self) -> fdo::Result<()> {
        if self.stopped.load(Ordering::SeqCst) {
            return Err(fdo::Error::Failed(
                "screen cast session is closed".to_owned(),
            ));
        }
        Ok(())
    }

    fn authorize_target(&self, target: &StreamTargetId) -> fdo::Result<()> {
        if !target_is_authorized(self.authorized_targets.as_deref(), target) {
            return Err(fdo::Error::AccessDenied(
                "target was not authorized by the screen cast picker".to_owned(),
            ));
        }
        Ok(())
    }

    async fn stop_internal(&self, server: &ObjectServer, ctxt: &SignalEmitter<'_>) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }

        if let Err(err) = Session::closed(ctxt).await {
            warn!("error emitting screen cast session Closed signal: {err:?}");
        }

        if let Err(err) = self.to_niri.send(ScreenCastToNiri::StopCast {
            session_id: self.id,
        }) {
            warn!("error sending StopCast to niri: {err:?}");
        }

        let streams = mem::take(&mut *self.streams.lock().unwrap());
        for (_, iface) in &streams {
            if let Err(err) = server
                .remove::<Stream, _>(iface.signal_emitter().path())
                .await
            {
                warn!("error removing screen cast stream object: {err:?}");
            }
        }

        match server.remove::<Session, _>(ctxt.path()).await {
            Ok(_) => {
                if let Some(sessions) = self.sessions.upgrade() {
                    sessions.lock().unwrap().remove(&self.id);
                }
            }
            Err(err) => {
                warn!("error removing screen cast session object: {err:?}");
            }
        }
    }
}

impl Stream {
    fn new(
        id: CastStreamId,
        session_id: CastSessionId,
        target: StreamTarget,
        cursor_mode: CursorMode,
        to_niri: calloop::channel::Sender<ScreenCastToNiri>,
    ) -> Self {
        Self {
            id,
            session_id,
            target,
            cursor_mode,
            was_started: Arc::new(AtomicBool::new(false)),
            to_niri,
        }
    }

    fn start(&self, ctxt: SignalEmitter<'static>) {
        if self.was_started.swap(true, Ordering::SeqCst) {
            return;
        }

        let msg = ScreenCastToNiri::StartCast {
            session_id: self.session_id,
            stream_id: self.id,
            target: self.target.make_id(),
            cursor_mode: self.cursor_mode,
            node_sink: NodeIdSink::MutterSignal(ctxt),
        };

        if let Err(err) = self.to_niri.send(msg) {
            warn!("error sending StartCast to niri: {err:?}");
        }
    }
}

impl StreamTarget {
    fn make_id(&self) -> StreamTargetId {
        match self {
            StreamTarget::Output(output) => StreamTargetId::Output {
                name: output.name.clone(),
            },
            StreamTarget::Window { id } => StreamTargetId::Window { id: *id },
        }
    }
}

fn sender_from_header(header: &Header<'_>) -> fdo::Result<OwnedUniqueName> {
    header
        .sender()
        .map(|sender| OwnedUniqueName::from(sender.to_owned()))
        .ok_or_else(|| fdo::Error::Failed("D-Bus call has no sender".to_owned()))
}

fn target_is_authorized(
    authorized_targets: Option<&HashSet<StreamTargetId>>,
    target: &StreamTargetId,
) -> bool {
    authorized_targets.is_none_or(|targets| targets.contains(target))
}

async fn monitor_disappeared_clients(
    conn: &zbus::Connection,
    sessions: SessionRegistry,
    portal: PortalScreenCast,
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
            error!("non-null new_owner should've been filtered out");
            continue;
        }

        let owner = OwnedUniqueName::from(name.to_owned());
        portal.client_disconnected(&owner);

        let owned_sessions = sessions
            .lock()
            .unwrap()
            .values()
            .filter(|(session, _)| session.owner == owner)
            .map(|(session, iface)| (session.clone(), iface.signal_emitter().clone()))
            .collect::<Vec<_>>();
        for (session, ctxt) in owned_sessions {
            session.stop_internal(conn.object_server(), &ctxt).await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenless_sessions_are_unrestricted_but_token_sessions_are_not() {
        let monitor = StreamTargetId::Output {
            name: "DP-1".to_owned(),
        };
        let window = StreamTargetId::Window { id: 10 };
        let authorized = HashSet::from([monitor.clone()]);

        assert!(target_is_authorized(None, &monitor));
        assert!(target_is_authorized(None, &window));
        assert!(target_is_authorized(Some(&authorized), &monitor));
        assert!(!target_is_authorized(Some(&authorized), &window));
    }
}
