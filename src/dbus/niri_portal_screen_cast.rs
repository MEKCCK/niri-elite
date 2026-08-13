use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use bitflags::bitflags;
use zbus::message::Header;
use zbus::names::OwnedUniqueName;
use zbus::zvariant::{DeserializeDict, OwnedObjectPath, SerializeDict, Type};
use zbus::{fdo, interface};

use super::mutter_screen_cast::{ScreenCastToNiri, StreamTargetId};

const RESPONSE_SUCCESS: u32 = 0;
const RESPONSE_CANCELLED: u32 = 1;
const RESPONSE_ERROR: u32 = 2;
const SELECTION_TOKEN_LEN: usize = 32;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PickerSourceTypes: u32 {
        const MONITOR = 1;
        const WINDOW = 2;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PickerCursorMode {
    Hidden = 1,
    Embedded = 2,
    Metadata = 4,
}

impl TryFrom<u32> for PickerCursorMode {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Hidden),
            2 => Ok(Self::Embedded),
            4 => Ok(Self::Metadata),
            _ => Err("cursor-mode must be hidden (1), embedded (2), or metadata (4)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u32)]
pub enum PickerPersistMode {
    None = 0,
    Transient = 1,
    Persistent = 2,
}

impl TryFrom<u32> for PickerPersistMode {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Transient),
            2 => Ok(Self::Persistent),
            _ => Err("persist-mode must be none (0), transient (1), or persistent (2)"),
        }
    }
}

#[derive(Debug, Default, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
struct PickSourcesOptionsDict {
    #[zvariant(rename = "app-id")]
    app_id: Option<String>,
    types: Option<u32>,
    multiple: Option<bool>,
    #[zvariant(rename = "cursor-mode")]
    cursor_mode: Option<u32>,
    #[zvariant(rename = "persist-mode")]
    persist_mode: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerOptions {
    pub app_id: String,
    pub source_types: PickerSourceTypes,
    pub multiple: bool,
    pub cursor_mode: PickerCursorMode,
    pub persist_mode: PickerPersistMode,
}

impl TryFrom<PickSourcesOptionsDict> for PickerOptions {
    type Error = &'static str;

    fn try_from(options: PickSourcesOptionsDict) -> Result<Self, Self::Error> {
        let types = options
            .types
            .unwrap_or_else(|| PickerSourceTypes::MONITOR.bits());
        let source_types = PickerSourceTypes::from_bits(types)
            .ok_or("types contains unsupported source type bits")?;
        if source_types.is_empty() {
            return Err("types must include at least one source type");
        }

        let cursor_mode = PickerCursorMode::try_from(options.cursor_mode.unwrap_or(1))?;
        let persist_mode = PickerPersistMode::try_from(options.persist_mode.unwrap_or(0))?;

        Ok(Self {
            app_id: options.app_id.unwrap_or_default(),
            source_types,
            multiple: options.multiple.unwrap_or(false),
            cursor_mode,
            persist_mode,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerRequestId {
    pub sender: OwnedUniqueName,
    pub handle: OwnedObjectPath,
}

pub struct PickSourcesRequest {
    pub id: PickerRequestId,
    pub options: PickerOptions,
    pub reply: PickSourcesReplySender,
}

impl PickSourcesRequest {
    pub fn fail(self, message: impl Into<String>) {
        let _ = self
            .reply
            .try_send(PickSourcesReply::Failed(message.into()));
    }
}

#[derive(Debug)]
pub enum PickSourcesReply {
    Selected(PickerSelection),
    Cancelled,
    Failed(String),
}

pub type PickSourcesReplySender = async_channel::Sender<PickSourcesReply>;

#[derive(Debug)]
pub struct PickerSelection {
    pub sources: Vec<StreamTargetId>,
    pub persist_mode: PickerPersistMode,
}

#[derive(Debug, Default, SerializeDict, Type)]
#[zvariant(signature = "dict")]
struct PickSourcesResults {
    sources: Option<Vec<(u32, PickerSourceProperties)>>,
    #[zvariant(rename = "persist-mode")]
    persist_mode: Option<u32>,
    #[zvariant(rename = "selection-token")]
    selection_token: Option<Vec<u8>>,
}

#[derive(Debug, SerializeDict, Type)]
#[zvariant(signature = "dict")]
struct PickerSourceProperties {
    connector: Option<String>,
    #[zvariant(rename = "window-id")]
    window_id: Option<u64>,
}

#[derive(Clone)]
pub struct PortalScreenCast {
    to_niri: calloop::channel::Sender<ScreenCastToNiri>,
    requests: Arc<Mutex<PickerRequestState>>,
    tokens: SelectionTokens,
}

#[derive(Default)]
struct PickerRequestState {
    pending: Option<PendingPickerRequest>,
}

struct PendingPickerRequest {
    id: PickerRequestId,
    reply: PickSourcesReplySender,
}

struct ActivePickerRequest {
    id: PickerRequestId,
    requests: Arc<Mutex<PickerRequestState>>,
    to_niri: calloop::channel::Sender<ScreenCastToNiri>,
    active: bool,
}

#[derive(Clone, Default)]
pub(super) struct SelectionTokens {
    grants: Arc<Mutex<HashMap<[u8; SELECTION_TOKEN_LEN], SelectionGrant>>>,
}

struct SelectionGrant {
    sender: OwnedUniqueName,
    targets: HashSet<StreamTargetId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConsumeTokenError {
    InvalidLength,
    Invalid,
}

#[interface(name = "org.niri.Portal.ScreenCast")]
impl PortalScreenCast {
    #[zbus(out_args("response", "results"))]
    async fn pick_sources(
        &self,
        #[zbus(header)] header: Header<'_>,
        request_handle: OwnedObjectPath,
        options: PickSourcesOptionsDict,
    ) -> fdo::Result<(u32, PickSourcesResults)> {
        let options = PickerOptions::try_from(options)
            .map_err(|err| fdo::Error::InvalidArgs(err.to_owned()))?;
        let id = PickerRequestId {
            sender: sender_from_header(&header)?,
            handle: request_handle,
        };

        let (reply, receiver) = async_channel::unbounded();
        if !self
            .requests
            .lock()
            .unwrap()
            .begin(id.clone(), reply.clone())
        {
            return Ok((RESPONSE_ERROR, PickSourcesResults::default()));
        }

        let mut active = ActivePickerRequest {
            id: id.clone(),
            requests: self.requests.clone(),
            to_niri: self.to_niri.clone(),
            active: true,
        };
        let request = PickSourcesRequest {
            id: id.clone(),
            options: options.clone(),
            reply,
        };
        if let Err(err) = self.to_niri.send(ScreenCastToNiri::PickSources(request)) {
            active.complete();
            return Err(fdo::Error::Failed(format!(
                "error sending picker request to niri: {err}"
            )));
        }

        let reply = match receiver.recv().await {
            Ok(reply) => reply,
            Err(err) => {
                active.complete();
                return Err(fdo::Error::Failed(format!(
                    "picker reply channel closed: {err}"
                )));
            }
        };

        if !active.complete() {
            return Ok((RESPONSE_CANCELLED, PickSourcesResults::default()));
        }

        let PickSourcesReply::Selected(selection) = reply else {
            if let PickSourcesReply::Failed(message) = reply {
                warn!("screen cast picker failed: {message}");
                return Ok((RESPONSE_ERROR, PickSourcesResults::default()));
            }
            return Ok((RESPONSE_CANCELLED, PickSourcesResults::default()));
        };

        let (sources, persist_mode) = match validate_selection(&options, selection) {
            Ok(selection) => selection,
            Err(err) => {
                warn!("screen cast picker returned an invalid selection: {err}");
                return Ok((RESPONSE_ERROR, PickSourcesResults::default()));
            }
        };
        let selection_token = self.tokens.issue(&id.sender, &sources).map_err(|err| {
            fdo::Error::Failed(format!("error generating selection token: {err}"))
        })?;

        Ok((
            RESPONSE_SUCCESS,
            PickSourcesResults {
                sources: Some(sources.into_iter().map(source_result).collect()),
                persist_mode: Some(persist_mode as u32),
                selection_token: Some(selection_token),
            },
        ))
    }

    async fn cancel(
        &self,
        #[zbus(header)] header: Header<'_>,
        request_handle: OwnedObjectPath,
    ) -> fdo::Result<()> {
        let id = PickerRequestId {
            sender: sender_from_header(&header)?,
            handle: request_handle,
        };
        self.cancel_request(&id);
        Ok(())
    }

    #[zbus(property)]
    async fn version(&self) -> u32 {
        1
    }
}

impl PortalScreenCast {
    pub(super) fn new(
        to_niri: calloop::channel::Sender<ScreenCastToNiri>,
        tokens: SelectionTokens,
    ) -> Self {
        Self {
            to_niri,
            requests: Arc::new(Mutex::new(PickerRequestState::default())),
            tokens,
        }
    }

    pub(super) fn client_disconnected(&self, sender: &OwnedUniqueName) {
        self.tokens.remove_sender(sender);

        let id = self.requests.lock().unwrap().pending_for_sender(sender);
        if let Some(id) = id {
            self.cancel_request(&id);
        }
    }

    fn cancel_request(&self, id: &PickerRequestId) {
        if self.requests.lock().unwrap().cancel(id) {
            if let Err(err) = self
                .to_niri
                .send(ScreenCastToNiri::CancelPickSources(id.clone()))
            {
                warn!("error sending picker cancellation to niri: {err}");
            }
        }
    }
}

impl PickerRequestState {
    fn begin(&mut self, id: PickerRequestId, reply: PickSourcesReplySender) -> bool {
        if self.pending.is_some() {
            return false;
        }

        self.pending = Some(PendingPickerRequest { id, reply });
        true
    }

    fn complete(&mut self, id: &PickerRequestId) -> bool {
        if !matches!(&self.pending, Some(pending) if pending.id == *id) {
            return false;
        }

        self.pending.take();
        true
    }

    fn cancel(&mut self, id: &PickerRequestId) -> bool {
        if !matches!(&self.pending, Some(pending) if pending.id == *id) {
            return false;
        }

        let pending = self.pending.take().unwrap();
        let _ = pending.reply.try_send(PickSourcesReply::Cancelled);
        true
    }

    fn pending_for_sender(&self, sender: &OwnedUniqueName) -> Option<PickerRequestId> {
        self.pending
            .as_ref()
            .filter(|pending| &pending.id.sender == sender)
            .map(|pending| pending.id.clone())
    }
}

impl ActivePickerRequest {
    fn complete(&mut self) -> bool {
        if !self.active {
            return false;
        }
        self.active = false;
        self.requests.lock().unwrap().complete(&self.id)
    }
}

impl Drop for ActivePickerRequest {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        if self.requests.lock().unwrap().cancel(&self.id) {
            let _ = self
                .to_niri
                .send(ScreenCastToNiri::CancelPickSources(self.id.clone()));
        }
    }
}

impl SelectionTokens {
    fn issue(
        &self,
        sender: &OwnedUniqueName,
        targets: &[StreamTargetId],
    ) -> Result<Vec<u8>, getrandom::Error> {
        loop {
            let mut token = [0; SELECTION_TOKEN_LEN];
            getrandom::fill(&mut token)?;

            let mut grants = self.grants.lock().unwrap();
            if let Entry::Vacant(entry) = grants.entry(token) {
                entry.insert(SelectionGrant {
                    sender: sender.clone(),
                    targets: targets.iter().cloned().collect(),
                });
                return Ok(token.to_vec());
            }
        }
    }

    pub(super) fn consume(
        &self,
        sender: &OwnedUniqueName,
        token: &[u8],
    ) -> Result<HashSet<StreamTargetId>, ConsumeTokenError> {
        let token: [u8; SELECTION_TOKEN_LEN] = token
            .try_into()
            .map_err(|_| ConsumeTokenError::InvalidLength)?;
        let mut grants = self.grants.lock().unwrap();
        let is_owner = grants
            .get(&token)
            .is_some_and(|grant| &grant.sender == sender);
        if !is_owner {
            return Err(ConsumeTokenError::Invalid);
        }

        Ok(grants.remove(&token).unwrap().targets)
    }

    fn remove_sender(&self, sender: &OwnedUniqueName) {
        self.grants
            .lock()
            .unwrap()
            .retain(|_, grant| &grant.sender != sender);
    }
}

fn sender_from_header(header: &Header<'_>) -> fdo::Result<OwnedUniqueName> {
    header
        .sender()
        .map(|sender| OwnedUniqueName::from(sender.to_owned()))
        .ok_or_else(|| fdo::Error::Failed("D-Bus call has no sender".to_owned()))
}

pub(super) fn validate_selection(
    options: &PickerOptions,
    selection: PickerSelection,
) -> Result<(Vec<StreamTargetId>, PickerPersistMode), &'static str> {
    if selection.sources.is_empty() {
        return Err("selection is empty");
    }
    if !options.multiple && selection.sources.len() != 1 {
        return Err("multiple sources were selected when multiple is false");
    }
    if selection.persist_mode > options.persist_mode {
        return Err("selection persist mode exceeds the requested mode");
    }

    let mut unique = HashSet::new();
    for source in &selection.sources {
        let allowed = match source {
            StreamTargetId::Output { .. } => {
                options.source_types.contains(PickerSourceTypes::MONITOR)
            }
            StreamTargetId::Window { .. } => {
                options.source_types.contains(PickerSourceTypes::WINDOW)
            }
        };
        if !allowed {
            return Err("selection contains a source type that was not requested");
        }
        if !unique.insert(source) {
            return Err("selection contains duplicate sources");
        }
    }

    Ok((selection.sources, selection.persist_mode))
}

fn source_result(source: StreamTargetId) -> (u32, PickerSourceProperties) {
    match source {
        StreamTargetId::Output { name } => (
            PickerSourceTypes::MONITOR.bits(),
            PickerSourceProperties {
                connector: Some(name),
                window_id: None,
            },
        ),
        StreamTargetId::Window { id } => (
            PickerSourceTypes::WINDOW.bits(),
            PickerSourceProperties {
                connector: None,
                window_id: Some(id),
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(sender: &str, handle: &str) -> PickerRequestId {
        PickerRequestId {
            sender: OwnedUniqueName::try_from(sender.to_owned()).unwrap(),
            handle: OwnedObjectPath::try_from(handle.to_owned()).unwrap(),
        }
    }

    fn valid_options() -> PickSourcesOptionsDict {
        PickSourcesOptionsDict {
            types: Some(PickerSourceTypes::MONITOR.bits()),
            ..Default::default()
        }
    }

    #[test]
    fn validates_picker_options() {
        let defaults = PickerOptions::try_from(PickSourcesOptionsDict::default()).unwrap();
        assert_eq!(defaults.source_types, PickerSourceTypes::MONITOR);
        assert!(!defaults.multiple);
        assert!(defaults.app_id.is_empty());
        assert_eq!(defaults.cursor_mode, PickerCursorMode::Hidden);
        assert_eq!(defaults.persist_mode, PickerPersistMode::None);

        let mut options = valid_options();
        options.types = Some(0);
        assert_eq!(
            PickerOptions::try_from(options).unwrap_err(),
            "types must include at least one source type"
        );

        let mut options = valid_options();
        options.types = Some(4);
        assert_eq!(
            PickerOptions::try_from(options).unwrap_err(),
            "types contains unsupported source type bits"
        );

        let mut options = valid_options();
        options.cursor_mode = Some(3);
        assert!(PickerOptions::try_from(options).is_err());

        let mut options = valid_options();
        options.persist_mode = Some(3);
        assert!(PickerOptions::try_from(options).is_err());

        let options = PickerOptions::try_from(valid_options()).unwrap();
        assert_eq!(options.source_types, PickerSourceTypes::MONITOR);
        assert!(!options.multiple);
        assert!(options.app_id.is_empty());
        assert_eq!(options.cursor_mode, PickerCursorMode::Hidden);
        assert_eq!(options.persist_mode, PickerPersistMode::None);
    }

    #[test]
    fn only_one_picker_request_can_be_pending() {
        let mut state = PickerRequestState::default();
        let first = id(":1.1", "/request/1");
        let second = id(":1.2", "/request/2");
        let (first_reply, _) = async_channel::unbounded();
        let (second_reply, _) = async_channel::unbounded();

        assert!(state.begin(first.clone(), first_reply));
        assert!(!state.begin(second.clone(), second_reply.clone()));
        assert!(state.complete(&first));
        assert!(state.begin(second, second_reply));
    }

    #[test]
    fn cancellation_matches_sender_and_handle_exactly_once() {
        let mut state = PickerRequestState::default();
        let request = id(":1.1", "/request/1");
        let wrong_sender = id(":1.2", "/request/1");
        let wrong_handle = id(":1.1", "/request/2");
        let (reply, receiver) = async_channel::unbounded();

        assert!(state.begin(request.clone(), reply));
        assert!(!state.cancel(&wrong_sender));
        assert!(!state.cancel(&wrong_handle));
        assert!(receiver.try_recv().is_err());

        assert!(state.cancel(&request));
        assert!(matches!(
            receiver.try_recv(),
            Ok(PickSourcesReply::Cancelled)
        ));
        assert!(!state.cancel(&request));
        assert!(!state.complete(&request));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn cancellation_wins_over_an_uncommitted_picker_reply() {
        let mut state = PickerRequestState::default();
        let request = id(":1.1", "/request/1");
        let (reply, receiver) = async_channel::unbounded();

        assert!(state.begin(request.clone(), reply.clone()));
        reply
            .try_send(PickSourcesReply::Selected(PickerSelection {
                sources: vec![StreamTargetId::Output {
                    name: "DP-1".to_owned(),
                }],
                persist_mode: PickerPersistMode::None,
            }))
            .unwrap();
        assert!(state.cancel(&request));

        assert!(matches!(
            receiver.try_recv(),
            Ok(PickSourcesReply::Selected(_))
        ));
        assert!(!state.complete(&request));
        assert!(matches!(
            receiver.try_recv(),
            Ok(PickSourcesReply::Cancelled)
        ));
    }

    #[test]
    fn picker_dictionary_signatures_match_the_protocol() {
        assert_eq!(PickSourcesOptionsDict::SIGNATURE.to_string(), "a{sv}");
        assert_eq!(PickSourcesResults::SIGNATURE.to_string(), "a{sv}");
        assert_eq!(
            Vec::<(u32, PickerSourceProperties)>::SIGNATURE.to_string(),
            "a(ua{sv})"
        );
        assert_eq!(Vec::<u8>::SIGNATURE.to_string(), "ay");
    }

    #[test]
    fn selection_tokens_are_sender_bound_and_one_use() {
        let tokens = SelectionTokens::default();
        let owner = id(":1.1", "/request/1").sender;
        let other = id(":1.2", "/request/2").sender;
        let targets = vec![StreamTargetId::Output {
            name: "DP-1".to_owned(),
        }];
        let token = tokens.issue(&owner, &targets).unwrap();

        assert_eq!(
            tokens.consume(&other, &token),
            Err(ConsumeTokenError::Invalid)
        );
        assert_eq!(
            tokens.consume(&owner, &[0; SELECTION_TOKEN_LEN - 1]),
            Err(ConsumeTokenError::InvalidLength)
        );

        let grant = tokens.consume(&owner, &token).unwrap();
        assert_eq!(grant, HashSet::from_iter(targets));
        assert_eq!(
            tokens.consume(&owner, &token),
            Err(ConsumeTokenError::Invalid)
        );
    }
}
