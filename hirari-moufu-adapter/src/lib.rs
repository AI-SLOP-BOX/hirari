//! Optional live project-state publishing to Moufu Integration Hub.

use moufu_protocol::{AppCapabilities, ChangeEvent, ClientMessage, EntityId, ServerMessage};
pub use moufu_protocol::{EntityDescriptor, LinkId, LinkInfo, LinkStatus, PayloadTransport};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

pub const HIRARI_PROJECT_ENTITY: &str = "hirari://workspace/current-project";
pub const HIRARI_PROJECT_DATA_TYPE: &str = "application/vnd.moufu.project+json";
pub const HIRARI_PROJECT_CONTRACT: &str = "hirari-project-layout-v2";
const MAX_SERVER_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CLIENT_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_QUEUED_SERVER_MESSAGES: usize = 4;
const MAX_QUEUED_APP_EVENTS: usize = 64;

/// Events received from Moufu for consumers that subscribe to linked data.
#[derive(Debug, Clone)]
pub enum MoufuEvent {
    ConnectionConfigured {
        enabled: bool,
    },
    Connected {
        server_version: String,
    },
    Disconnected {
        reason: String,
    },
    LinkStatusChanged(LinkInfo),
    LinkDestroyed {
        link_id: LinkId,
    },
    EntityList(Vec<EntityDescriptor>),
    LinkList(Vec<LinkInfo>),
    EntityUpdated(ChangeEvent),
    EntityState {
        entity_id: EntityId,
        version: u64,
        payload: PayloadTransport,
    },
    ServerNotice {
        code: String,
        message: String,
    },
}

impl MoufuEvent {
    /// Short, non-sensitive status suitable for an application's status bar.
    pub fn status_message(&self) -> Option<&'static str> {
        match self {
            Self::ConnectionConfigured { enabled: true } => Some("MOUFU CONNECTING"),
            Self::ConnectionConfigured { enabled: false } => Some("MOUFU NOT CONFIGURED"),
            Self::Connected { .. } => Some("MOUFU CONNECTED"),
            Self::Disconnected { .. } => Some("MOUFU RECONNECTING"),
            Self::LinkStatusChanged(link) if link.status == LinkStatus::Active => {
                Some("MOUFU LINK ACTIVE")
            }
            Self::LinkStatusChanged(_) => Some("MOUFU LINK NEEDS ATTENTION"),
            Self::LinkDestroyed { .. } => Some("MOUFU LINK REMOVED"),
            Self::EntityUpdated(_) => Some("MOUFU UPDATE RECEIVED"),
            Self::EntityState { .. } => Some("MOUFU STATE RECEIVED"),
            Self::EntityList(_) | Self::LinkList(_) => None,
            Self::ServerNotice { .. } => Some("MOUFU REQUEST NEEDS ATTENTION"),
        }
    }
}

#[derive(Debug, Clone)]
struct ProjectSnapshot {
    version: u64,
    is_transient: bool,
    layout_json: String,
    recording_input_channels: Vec<(u32, Vec<u16>)>,
}

/// Non-blocking handle for publishing the current Hirari project to Moufu.
pub struct MoufuPublisher {
    address: Arc<Mutex<Option<String>>>,
    latest: Arc<Mutex<Option<ProjectSnapshot>>>,
    next_version: Arc<AtomicU64>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    owners: Arc<AtomicUsize>,
    events: Arc<MoufuEventQueue>,
    requests: mpsc::SyncSender<QueuedMoufuRequest>,
}

impl Clone for MoufuPublisher {
    fn clone(&self) -> Self {
        self.owners.fetch_add(1, Ordering::Relaxed);
        Self {
            address: Arc::clone(&self.address),
            latest: Arc::clone(&self.latest),
            next_version: Arc::clone(&self.next_version),
            stop: Arc::clone(&self.stop),
            owners: Arc::clone(&self.owners),
            events: Arc::clone(&self.events),
            requests: self.requests.clone(),
        }
    }
}

impl Drop for MoufuPublisher {
    fn drop(&mut self) {
        if self.owners.fetch_sub(1, Ordering::AcqRel) == 1 {
            // The connection worker is detached so dropping the UI handle
            // must explicitly end its reconnect loop. The worker observes
            // this flag between bounded socket waits and closes its stream.
            self.stop.store(true, Ordering::Release);
        }
    }
}

struct MoufuEventQueue {
    queue: Mutex<VecDeque<MoufuEvent>>,
    ready: Condvar,
    generation: AtomicU64,
}

impl Default for MoufuEventQueue {
    fn default() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            generation: AtomicU64::new(0),
        }
    }
}

impl MoufuEventQueue {
    fn reset(&self, enabled: bool) {
        let Ok(mut queue) = self.queue.lock() else {
            self.generation.fetch_add(1, Ordering::AcqRel);
            return;
        };
        self.generation.fetch_add(1, Ordering::AcqRel);
        queue.clear();
        queue.push_back(MoufuEvent::ConnectionConfigured { enabled });
        self.ready.notify_one();
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::Acquire) == generation
    }

    fn push(&self, generation: u64, event: MoufuEvent) -> bool {
        let Ok(mut queue) = self.queue.lock() else {
            return false;
        };
        if !self.is_current(generation) {
            return true;
        }

        if let Some((index, current_version, current_transient)) =
            queue.iter().enumerate().find_map(|(index, queued)| {
                let event_entity = state_event_entity(&event)?;
                (state_event_entity(queued) == Some(event_entity)).then(|| {
                    (
                        index,
                        state_event_version(queued).unwrap_or_default(),
                        state_event_is_transient(queued),
                    )
                })
            })
        {
            let next_version = state_event_version(&event).unwrap_or_default();
            let next_transient = state_event_is_transient(&event);
            if next_version > current_version
                || (next_version == current_version && current_transient && !next_transient)
            {
                queue[index] = event;
                self.ready.notify_one();
            }
            return true;
        }

        if let Some(index) = queue.iter().position(|queued| {
            matches!(
                (&event, queued),
                (MoufuEvent::EntityList(_), MoufuEvent::EntityList(_))
                    | (MoufuEvent::LinkList(_), MoufuEvent::LinkList(_))
            )
        }) {
            queue[index] = event;
            self.ready.notify_one();
            return true;
        }

        if queue.len() >= MAX_QUEUED_APP_EVENTS {
            if let Some(index) = queue
                .iter()
                .position(|queued| state_event_entity(queued).is_some())
            {
                queue.remove(index);
            } else if state_event_entity(&event).is_some() {
                return false;
            } else {
                queue.pop_front();
            }
        }

        queue.push_back(event);
        self.ready.notify_one();
        true
    }

    fn drain(&self) -> Vec<MoufuEvent> {
        let Ok(mut queue) = self.queue.lock() else {
            return Vec::new();
        };
        std::mem::take(&mut *queue).into()
    }

    fn recv_timeout(&self, timeout: Duration) -> Option<MoufuEvent> {
        let queue = self.queue.lock().ok()?;
        let (mut queue, _) = self
            .ready
            .wait_timeout_while(queue, timeout, |events| events.is_empty())
            .unwrap_or_else(PoisonError::into_inner);
        queue.pop_front()
    }
}

fn state_event_entity(event: &MoufuEvent) -> Option<&EntityId> {
    match event {
        MoufuEvent::EntityUpdated(change) => Some(&change.entity_id),
        MoufuEvent::EntityState { entity_id, .. } => Some(entity_id),
        _ => None,
    }
}

fn state_event_version(event: &MoufuEvent) -> Option<u64> {
    match event {
        MoufuEvent::EntityUpdated(change) => Some(change.version),
        MoufuEvent::EntityState { version, .. } => Some(*version),
        _ => None,
    }
}

fn state_event_is_transient(event: &MoufuEvent) -> bool {
    match event {
        MoufuEvent::EntityUpdated(change) => change.is_transient,
        MoufuEvent::EntityState { .. } => false,
        _ => false,
    }
}

enum MoufuRequest {
    ListEntities,
    ListLinks,
    CreateLink { source: EntityId, target: EntityId },
    DestroyLink(LinkId),
}

struct QueuedMoufuRequest {
    generation: u64,
    request: MoufuRequest,
}

impl MoufuPublisher {
    /// Start the desktop connector using `HIRARI_MOUFU_ADDR` when present.
    /// The returned idle handle also supports in-app connection setup.
    pub fn start_from_env() -> Option<Self> {
        if std::env::var("HIRARI_HEADLESS").as_deref() == Ok("1")
            || std::env::var("HIRARI_UI_SMOKE").as_deref() == Ok("1")
        {
            return None;
        }
        let publisher = Self::new_disconnected().ok()?;
        if let Ok(address) = std::env::var("HIRARI_MOUFU_ADDR") {
            if !address.trim().is_empty() {
                let _ = publisher.set_address(Some(&address));
            }
        }
        Some(publisher)
    }

    /// Start the connector for an explicit address. This is also useful for
    /// embedding Hirari with an app-managed settings surface.
    pub fn connect_to(address: &str) -> io::Result<Self> {
        let address = address.trim().to_owned();
        if address.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Moufu address cannot be empty",
            ));
        }
        let publisher = Self::new_disconnected()?;
        publisher.set_address(Some(&address))?;
        Ok(publisher)
    }

    /// Create an idle connector that can be configured from an application's
    /// settings UI without restarting the process.
    pub fn new_disconnected() -> io::Result<Self> {
        let address = Arc::new(Mutex::new(None));
        let latest = Arc::new(Mutex::new(None));
        let starting_version = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(u64::MAX as u128) as u64;
        let next_version = Arc::new(AtomicU64::new(starting_version));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let owners = Arc::new(AtomicUsize::new(1));
        let events = Arc::new(MoufuEventQueue::default());
        let (request_tx, request_rx) = mpsc::sync_channel(8);
        let worker_latest = Arc::clone(&latest);
        let worker_address = Arc::clone(&address);
        let worker_stop = Arc::clone(&stop);
        let worker_events = Arc::clone(&events);
        thread::Builder::new()
            .name("hirari-moufu-link".to_owned())
            .spawn(move || {
                run_worker(
                    worker_address,
                    worker_latest,
                    worker_stop,
                    worker_events,
                    request_rx,
                )
            })?;

        Ok(Self {
            address,
            latest,
            next_version,
            stop,
            owners,
            events,
            requests: request_tx,
        })
    }

    /// Change the Hub address while the connector is running. An empty
    /// address cleanly disconnects without stopping publication of the latest
    /// snapshot, so reconnecting can immediately restore the project state.
    pub fn set_address(&self, address: Option<&str>) -> io::Result<()> {
        let address = address
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .map(str::to_owned);
        if address.as_ref().is_some_and(|address| address.len() > 1024) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Moufu address is too long",
            ));
        }
        *self
            .address
            .lock()
            .map_err(|_| io::Error::other("Moufu address state poisoned"))? = address;
        let enabled = self
            .address
            .lock()
            .map(|address| address.is_some())
            .unwrap_or(false);
        self.events.reset(enabled);
        Ok(())
    }

    /// Publish a serialized project layout. Repeated edits coalesce to the
    /// newest snapshot while the network worker is busy or reconnecting.
    pub fn publish_layout(&self, layout_json: &str, is_transient: bool) {
        self.publish_layout_owned(layout_json.to_owned(), is_transient);
    }

    /// Queue an owned layout string without parsing it on the caller thread.
    /// This is the preferred entry point for the UI, which already receives
    /// owned JSON from Core and must not normalize large sessions inline.
    pub fn publish_layout_owned(&self, layout_json: String, is_transient: bool) {
        self.publish_layout_with_recording_inputs_owned(layout_json, Vec::new(), is_transient);
    }

    /// Queue an owned layout and UI-owned recording input assignments. The
    /// adapter applies those small UI values after parsing the layout on its
    /// worker, avoiding a second project-sized parse/serialization on the UI
    /// thread.
    pub fn publish_layout_with_recording_inputs_owned(
        &self,
        layout_json: String,
        recording_input_channels: Vec<(u32, Vec<u16>)>,
        is_transient: bool,
    ) {
        if layout_json.trim().is_empty() {
            log::warn!("Hirari could not publish an empty project layout to Moufu");
            return;
        }
        match self.latest.lock() {
            Ok(mut latest) => {
                let version = self.next_version.fetch_add(1, Ordering::Relaxed);
                *latest = Some(ProjectSnapshot {
                    version,
                    is_transient,
                    layout_json,
                    recording_input_channels,
                });
            }
            Err(_) => log::warn!("Hirari Moufu publisher state is unavailable"),
        }
    }

    /// Re-announce the latest layout with a fresh entity version. Moufu
    /// rejects duplicate versions, so this is needed when a new link becomes
    /// active without any project edits since the previous link was created.
    pub fn republish_latest_layout(&self) -> bool {
        let Ok(mut latest) = self.latest.lock() else {
            log::warn!("Hirari Moufu publisher state is unavailable");
            return false;
        };
        let Some(snapshot) = latest.as_mut() else {
            return false;
        };
        snapshot.version = self.next_version.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Discard the cached project snapshot when the caller cannot produce a
    /// trustworthy replacement. This prevents a later link from republishing
    /// the previous project's payload under the current session.
    pub fn discard_latest_layout(&self) -> bool {
        match self.latest.lock() {
            Ok(mut latest) => latest.take().is_some(),
            Err(_) => {
                log::warn!("Hirari Moufu publisher state is unavailable");
                false
            }
        }
    }

    /// Stop the background connection worker. Clones share the same worker.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn list_entities(&self) -> io::Result<()> {
        self.enqueue_request(MoufuRequest::ListEntities)
    }

    pub fn list_links(&self) -> io::Result<()> {
        self.enqueue_request(MoufuRequest::ListLinks)
    }

    pub fn create_link(&self, source: &str, target: &str) -> io::Result<()> {
        let source = source.trim();
        let target = target.trim();
        if source.is_empty() || target.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Moufu link endpoints cannot be empty",
            ));
        }
        self.enqueue_request(MoufuRequest::CreateLink {
            source: EntityId::from(source),
            target: EntityId::from(target),
        })
    }

    pub fn destroy_link(&self, link_id: &str) -> io::Result<()> {
        let value = Value::String(link_id.trim().to_owned());
        let link_id = serde_json::from_value::<LinkId>(value)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        self.enqueue_request(MoufuRequest::DestroyLink(link_id))
    }

    fn enqueue_request(&self, request: MoufuRequest) -> io::Result<()> {
        self.requests
            .try_send(QueuedMoufuRequest {
                generation: self.events.generation.load(Ordering::Acquire),
                request,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "Moufu request queue is full")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "Moufu worker is stopped")
                }
            })
    }

    /// Drain remote state and link notifications received from Moufu.
    /// Applications should call this regularly while the connector is active.
    pub fn drain_events(&self) -> Vec<MoufuEvent> {
        self.events.drain()
    }

    /// Wait for one remote event. This is useful for hosts that do not have a
    /// regular UI timer to drain notifications.
    pub fn recv_event_timeout(&self, timeout: Duration) -> Option<MoufuEvent> {
        self.events.recv_timeout(timeout)
    }
}

fn redact_private_fields(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|key, _| !is_private_project_field(key));
            for child in fields.values_mut() {
                redact_private_fields(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_private_fields(item);
            }
        }
        _ => {}
    }
}

fn is_private_project_field(key: &str) -> bool {
    // Normalize separators so old snake_case keys and future camelCase keys
    // receive the same treatment (e.g. plugin_state_hex/pluginStateHex).
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized.contains("path")
        || (normalized.contains("plugin") && normalized.contains("state"))
        || matches!(normalized.as_str(), "stateblob" | "guistate")
}

fn resolve_addresses(address: &str) -> io::Result<Vec<SocketAddr>> {
    let mut addresses = address.to_socket_addrs()?.collect::<Vec<_>>();
    addresses.dedup();
    if addresses.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Moufu address resolved empty",
        ));
    }
    Ok(addresses)
}

fn write_message<T: Serialize>(stream: &mut TcpStream, message: &T) -> io::Result<()> {
    serde_json::to_writer(&mut *stream, message).map_err(io::Error::other)?;
    stream.write_all(b"\n")?;
    stream.flush()
}

struct StreamShutdown(TcpStream);

impl Drop for StreamShutdown {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

enum WireServerMessage {
    Protocol(ServerMessage),
    LinkDestroyed { link_id: LinkId },
    EntityList(Vec<EntityDescriptor>),
    LinkList(Vec<LinkInfo>),
}

fn decode_server_message(frame: &[u8]) -> Result<WireServerMessage, serde_json::Error> {
    let value = serde_json::from_slice::<Value>(frame)?;
    if value.get("type").and_then(Value::as_str) == Some("LinkDestroyed") {
        let link_id = value
            .get("data")
            .and_then(|data| data.get("link_id"))
            .cloned()
            .map(serde_json::from_value::<LinkId>)
            .transpose()?
            .ok_or_else(|| {
                serde_json::Error::io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Moufu LinkDestroyed message has no link_id",
                ))
            })?;
        return Ok(WireServerMessage::LinkDestroyed { link_id });
    }
    if value.get("type").and_then(Value::as_str) == Some("EntityList") {
        let entities = value
            .get("data")
            .cloned()
            .map(serde_json::from_value::<Vec<EntityDescriptor>>)
            .transpose()?
            .ok_or_else(|| {
                serde_json::Error::io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Moufu EntityList message has no data",
                ))
            })?;
        return Ok(WireServerMessage::EntityList(entities));
    }
    if value.get("type").and_then(Value::as_str) == Some("LinkList") {
        let links = value
            .get("data")
            .cloned()
            .map(serde_json::from_value::<Vec<LinkInfo>>)
            .transpose()?
            .ok_or_else(|| {
                serde_json::Error::io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Moufu LinkList message has no data",
                ))
            })?;
        return Ok(WireServerMessage::LinkList(links));
    }
    serde_json::from_value::<ServerMessage>(value).map(WireServerMessage::Protocol)
}

fn spawn_server_reader(reader: TcpStream) -> mpsc::Receiver<io::Result<WireServerMessage>> {
    let (tx, rx) = mpsc::sync_channel(MAX_QUEUED_SERVER_MESSAGES);
    let _ = thread::Builder::new()
        .name("hirari-moufu-reader".to_owned())
        .spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut line = Vec::with_capacity(4096);
            loop {
                let message =
                    match read_bounded_frame(&mut reader, &mut line, MAX_SERVER_MESSAGE_BYTES) {
                        Ok(0) => Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "Moufu disconnected",
                        )),
                        Ok(_) if line.iter().all(u8::is_ascii_whitespace) => {
                            line.clear();
                            continue;
                        }
                        Ok(_) => decode_server_message(&line)
                            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
                        Err(error) => Err(error),
                    };
                line.clear();
                let should_stop = message.is_err();
                if tx.send(message).is_err() || should_stop {
                    return;
                }
            }
        });
    rx
}

fn read_bounded_frame<R: BufRead>(
    reader: &mut R,
    frame: &mut Vec<u8>,
    max_bytes: usize,
) -> io::Result<usize> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(0)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Moufu closed in the middle of a message",
                ))
            };
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |newline| newline + 1);
        if frame.len().saturating_add(take) > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Moufu message exceeds the configured size limit",
            ));
        }
        let complete = available[take - 1] == b'\n';
        frame.extend_from_slice(&available[..take]);
        reader.consume(take);
        if complete {
            return Ok(frame.len());
        }
    }
}

fn receive_message(
    messages: &mpsc::Receiver<io::Result<WireServerMessage>>,
    wait: Duration,
) -> io::Result<Option<WireServerMessage>> {
    match messages.recv_timeout(wait) {
        Ok(message) => message.map(Some),
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Moufu reader stopped",
        )),
    }
}

fn run_worker(
    address: Arc<Mutex<Option<String>>>,
    latest: Arc<Mutex<Option<ProjectSnapshot>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    events: Arc<MoufuEventQueue>,
    requests: mpsc::Receiver<QueuedMoufuRequest>,
) {
    while !stop.load(Ordering::Acquire) {
        let generation = events.generation.load(Ordering::Acquire);
        let configured_address = address.lock().ok().and_then(|address| address.clone());
        let Some(configured_address) = configured_address else {
            thread::sleep(Duration::from_millis(100));
            continue;
        };
        let result = resolve_addresses(&configured_address).and_then(|resolved| {
            connect_and_publish(
                resolved,
                &latest,
                &stop,
                &address,
                &configured_address,
                generation,
                &events,
                &requests,
            )
        });
        let stale_generation = !events.is_current(generation);
        let address_changed = address
            .lock()
            .map(|current| current.as_deref() != Some(configured_address.as_str()))
            .unwrap_or(true);
        if stale_generation || address_changed {
            continue;
        }
        match result {
            Ok(()) => log::info!("Moufu closed the Hirari integration connection"),
            Err(error) => {
                publish_event(
                    &events,
                    generation,
                    MoufuEvent::Disconnected {
                        reason: error.to_string(),
                    },
                );
                log::debug!("Hirari Moufu connection retry: {error}");
            }
        }
        for _ in 0..20 {
            if stop.load(Ordering::Acquire) {
                return;
            }
            if !events.is_current(generation)
                || address
                    .lock()
                    .map(|current| current.as_deref() != Some(configured_address.as_str()))
                    .unwrap_or(true)
            {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn connect_and_publish(
    addresses: Vec<SocketAddr>,
    latest: &Arc<Mutex<Option<ProjectSnapshot>>>,
    stop: &std::sync::atomic::AtomicBool,
    configured_address: &Arc<Mutex<Option<String>>>,
    connected_address: &str,
    generation: u64,
    events: &MoufuEventQueue,
    requests: &mpsc::Receiver<QueuedMoufuRequest>,
) -> io::Result<()> {
    let mut connected = None;
    let mut last_connect_error = None;
    for address in addresses {
        if stop.load(Ordering::Acquire)
            || !events.is_current(generation)
            || configured_address
                .lock()
                .map(|current| current.as_deref() != Some(connected_address))
                .unwrap_or(true)
        {
            return Ok(());
        }
        match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
            Ok(stream) => {
                connected = Some((address, stream));
                break;
            }
            Err(error) => last_connect_error = Some(error),
        }
    }
    let (address, stream) = connected.ok_or_else(|| {
        last_connect_error.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "no Moufu address was attempted",
            )
        })
    })?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut writer = stream.try_clone()?;
    let reader = stream.try_clone()?;
    let _shutdown_guard = StreamShutdown(stream);
    let messages = spawn_server_reader(reader);

    write_message(
        &mut writer,
        &ClientMessage::Handshake {
            app_name: "Hirari".to_owned(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            capabilities: AppCapabilities {
                supports_live_link: true,
                supports_push_edits: false,
                supports_delta_sync: false,
                supports_link_restoration: true,
                supported_data_types: vec![HIRARI_PROJECT_DATA_TYPE.to_owned()],
            },
        },
    )?;

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut acknowledged = false;
    while Instant::now() < deadline {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if configured_address
            .lock()
            .map(|current| current.as_deref() != Some(connected_address))
            .unwrap_or(true)
            || !events.is_current(generation)
        {
            return Ok(());
        }
        write_queued_requests(&mut writer, requests, generation)?;
        match receive_message(&messages, Duration::from_millis(100))? {
            None => continue,
            Some(WireServerMessage::Protocol(ServerMessage::HandshakeAck {
                server_version,
                ..
            })) => {
                publish_event(events, generation, MoufuEvent::Connected { server_version });
                acknowledged = true;
                break;
            }
            Some(WireServerMessage::Protocol(ServerMessage::Error { code, message })) => {
                return Err(io::Error::other(format!(
                    "Moufu handshake rejected ({code}): {message}"
                )));
            }
            Some(_) => {}
        }
    }
    if !acknowledged {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Moufu handshake timed out",
        ));
    }

    log::info!("Hirari connected to Moufu at {address}");
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("contract".to_owned(), HIRARI_PROJECT_CONTRACT.to_owned());
    metadata.insert(
        "edit-direction".to_owned(),
        "hirari-to-linked-app".to_owned(),
    );
    write_message(
        &mut writer,
        &ClientMessage::RegisterEntity(EntityDescriptor {
            id: EntityId::from(HIRARI_PROJECT_ENTITY),
            name: "Current Hirari Project".to_owned(),
            data_type: HIRARI_PROJECT_DATA_TYPE.to_owned(),
            source_app: "Hirari".to_owned(),
            metadata,
        }),
    )?;

    // The pinned wire crate predates Moufu's read-only catalog messages. Send
    // their stable tagged JSON forms after the handshake so older Hubs can
    // reject them as ordinary requests without aborting connection setup.
    write_message(&mut writer, &serde_json::json!({"type": "ListEntities"}))?;
    write_message(&mut writer, &serde_json::json!({"type": "ListLinks"}))?;

    let mut last_sent_version = 0;
    let mut last_ping = Instant::now();
    let mut last_pong = Instant::now();
    let mut inbound_links = HashMap::new();
    let mut outbound_links = HashSet::new();
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if configured_address
            .lock()
            .map(|current| current.as_deref() != Some(connected_address))
            .unwrap_or(true)
            || !events.is_current(generation)
        {
            return Ok(());
        }
        write_queued_requests(&mut writer, requests, generation)?;
        let snapshot = if outbound_links.is_empty() {
            None
        } else {
            latest
                .lock()
                .map_err(|_| io::Error::other("publisher state poisoned"))?
                .as_ref()
                .filter(|snapshot| snapshot.version > last_sent_version)
                .cloned()
        };
        if let Some(snapshot) = snapshot {
            let mut payload = match serde_json::from_str::<Value>(&snapshot.layout_json) {
                Ok(payload) => payload,
                Err(error) => {
                    log::warn!(
                        "Hirari could not publish invalid project layout JSON to Moufu: {error}"
                    );
                    last_sent_version = snapshot.version;
                    continue;
                }
            };
            apply_recording_input_channels(&mut payload, &snapshot.recording_input_channels);
            redact_private_fields(&mut payload);
            let event = ChangeEvent {
                entity_id: EntityId::from(HIRARI_PROJECT_ENTITY),
                version: snapshot.version,
                is_transient: snapshot.is_transient,
                timestamp: chrono::Utc::now(),
                payload: PayloadTransport::Inline(payload),
                delta: None,
            };
            write_change_event(&mut writer, events, generation, event)?;
            last_sent_version = snapshot.version;
        }

        if last_ping.elapsed() >= Duration::from_secs(3) {
            write_message(&mut writer, &ClientMessage::Ping)?;
            last_ping = Instant::now();
        }
        if last_pong.elapsed() >= Duration::from_secs(12) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Moufu heartbeat timed out",
            ));
        }

        match receive_message(&messages, Duration::from_millis(100))? {
            None => continue,
            Some(WireServerMessage::LinkDestroyed { link_id }) => {
                inbound_links.remove(&link_id);
                outbound_links.remove(&link_id);
                publish_event(events, generation, MoufuEvent::LinkDestroyed { link_id });
            }
            Some(WireServerMessage::EntityList(entities)) => {
                publish_event(events, generation, MoufuEvent::EntityList(entities));
            }
            Some(WireServerMessage::LinkList(links)) => {
                publish_event(events, generation, MoufuEvent::LinkList(links));
            }
            Some(WireServerMessage::Protocol(ServerMessage::Pong)) => last_pong = Instant::now(),
            Some(WireServerMessage::Protocol(ServerMessage::LinkStatusChanged(link))) => {
                log_link_status(&link);
                if link.source_entity.0 == HIRARI_PROJECT_ENTITY {
                    if link.status == LinkStatus::Active
                        && link.target_entity.0 != HIRARI_PROJECT_ENTITY
                    {
                        outbound_links.insert(link.link_id);
                    } else {
                        outbound_links.remove(&link.link_id);
                    }
                }
                // A restored or newly created source -> Hirari link may have
                // been established before this connection came back. Request
                // the source's current snapshot so consumers don't have to
                // wait for its next edit before learning its state.
                if link.target_entity.0 == HIRARI_PROJECT_ENTITY {
                    let was_source_linked = inbound_links
                        .values()
                        .any(|source| source == &link.source_entity);
                    if link.status == LinkStatus::Active
                        && link.source_entity.0 != HIRARI_PROJECT_ENTITY
                    {
                        inbound_links.insert(link.link_id, link.source_entity.clone());
                    } else {
                        inbound_links.remove(&link.link_id);
                    }
                    let is_source_linked = inbound_links
                        .values()
                        .any(|source| source == &link.source_entity);
                    if !was_source_linked && is_source_linked {
                        write_message(
                            &mut writer,
                            &ClientMessage::PullEntity {
                                entity_id: link.source_entity.clone(),
                            },
                        )?;
                    }
                }
                publish_event(events, generation, MoufuEvent::LinkStatusChanged(link));
            }
            Some(WireServerMessage::Protocol(ServerMessage::EntityUpdated(event))) => {
                if !inbound_links
                    .values()
                    .any(|source| source == &event.entity_id)
                {
                    log::debug!(
                        "Ignored Moufu update for unlinked entity {}",
                        event.entity_id
                    );
                    continue;
                }
                log::debug!(
                    "Moufu delivered linked entity {} at version {}",
                    event.entity_id,
                    event.version
                );
                publish_event(events, generation, MoufuEvent::EntityUpdated(event));
            }
            Some(WireServerMessage::Protocol(ServerMessage::EntityState {
                entity_id,
                version,
                payload,
            })) => {
                if !inbound_links.values().any(|source| source == &entity_id) {
                    log::debug!("Ignored Moufu state for unlinked entity {}", entity_id);
                    continue;
                }
                log::debug!("Moufu returned entity {} at version {}", entity_id, version);
                publish_event(
                    events,
                    generation,
                    MoufuEvent::EntityState {
                        entity_id,
                        version,
                        payload,
                    },
                );
            }
            Some(WireServerMessage::Protocol(ServerMessage::Error { code, message })) => {
                log::warn!("Moufu rejected a Hirari request ({code}): {message}");
                publish_event(
                    events,
                    generation,
                    MoufuEvent::ServerNotice { code, message },
                );
            }
            Some(WireServerMessage::Protocol(ServerMessage::HandshakeAck { .. })) => {}
        }
    }
}

fn apply_recording_input_channels(payload: &mut Value, assignments: &[(u32, Vec<u16>)]) {
    let tracks = match payload {
        Value::Array(tracks) => Some(tracks),
        Value::Object(root) => root.get_mut("tracks").and_then(Value::as_array_mut),
        _ => None,
    };
    let Some(tracks) = tracks else {
        return;
    };
    for track in tracks {
        let Some(track_id) = track
            .get("id")
            .and_then(Value::as_u64)
            .and_then(|id| u32::try_from(id).ok())
        else {
            continue;
        };
        let Some((_, channels)) = assignments.iter().find(|(id, _)| *id == track_id) else {
            continue;
        };
        if let Some(track_object) = track.as_object_mut() {
            track_object.insert(
                "recording_input_channels".to_owned(),
                serde_json::json!(channels),
            );
        }
    }
}

fn publish_event(events: &MoufuEventQueue, generation: u64, event: MoufuEvent) {
    if !events.push(generation, event) {
        log::warn!("Hirari Moufu event queue is full; could not retain another state snapshot");
    }
}

fn write_queued_requests(
    writer: &mut TcpStream,
    requests: &mpsc::Receiver<QueuedMoufuRequest>,
    generation: u64,
) -> io::Result<()> {
    while let Ok(queued) = requests.try_recv() {
        if queued.generation != generation {
            continue;
        }
        match queued.request {
            MoufuRequest::ListEntities => {
                write_message(writer, &serde_json::json!({"type": "ListEntities"}))?;
            }
            MoufuRequest::ListLinks => {
                write_message(writer, &serde_json::json!({"type": "ListLinks"}))?;
            }
            MoufuRequest::CreateLink { source, target } => {
                write_message(
                    writer,
                    &ClientMessage::CreateLink {
                        source_entity: source,
                        target_entity: target,
                    },
                )?;
            }
            MoufuRequest::DestroyLink(link_id) => {
                write_message(writer, &ClientMessage::DestroyLink { link_id })?;
            }
        }
    }
    Ok(())
}

struct BoundedMessageBuffer {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl Write for BoundedMessageBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Moufu message exceeds the configured size limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write_change_event(
    writer: &mut TcpStream,
    events: &MoufuEventQueue,
    generation: u64,
    event: ChangeEvent,
) -> io::Result<()> {
    let mut message = BoundedMessageBuffer {
        bytes: Vec::with_capacity(64 * 1024),
        limit: MAX_CLIENT_MESSAGE_BYTES.saturating_sub(1),
        exceeded: false,
    };
    match serde_json::to_writer(&mut message, &ClientMessage::NotifyChange(event)) {
        Ok(()) => {
            message.bytes.push(b'\n');
            writer.write_all(&message.bytes)?;
            writer.flush()
        }
        Err(_) if message.exceeded => {
            publish_event(
                events,
                generation,
                MoufuEvent::ServerNotice {
                    code: "PROJECT_SNAPSHOT_TOO_LARGE".to_owned(),
                    message: format!(
                        "Project snapshot exceeds Moufu's {} MiB message limit and was not sent",
                        MAX_CLIENT_MESSAGE_BYTES / (1024 * 1024)
                    ),
                },
            );
            Ok(())
        }
        Err(error) => Err(io::Error::other(error)),
    }
}

fn log_link_status(link: &LinkInfo) {
    match link.status {
        LinkStatus::Active => log::info!(
            "Moufu link active: {} -> {}",
            link.source_entity,
            link.target_entity
        ),
        status => log::info!("Moufu link {}: {:?}", link.link_id, status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moufu_protocol::AppSessionId;
    use std::net::TcpListener;
    use std::sync::mpsc::channel;

    fn read_client_message(reader: &mut BufReader<TcpStream>) -> ClientMessage {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn server_frames_are_bounded_and_require_a_complete_newline() {
        let mut reader = BufReader::new(std::io::Cursor::new(b"abc\n"));
        let mut frame = Vec::new();
        assert_eq!(read_bounded_frame(&mut reader, &mut frame, 4).unwrap(), 4);
        assert_eq!(frame, b"abc\n");

        let mut reader = BufReader::new(std::io::Cursor::new(b"oversized\n"));
        let error = read_bounded_frame(&mut reader, &mut Vec::new(), 4).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let mut reader = BufReader::new(std::io::Cursor::new(b"partial"));
        let error = read_bounded_frame(&mut reader, &mut Vec::new(), 16).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn publisher_registers_hirari_and_sends_live_project_snapshot() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (observed_tx, observed_rx) = channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let handshake = read_client_message(&mut reader);
            observed_tx.send(handshake.clone()).unwrap();
            assert!(matches!(
                handshake,
                ClientMessage::Handshake { app_name, .. } if app_name == "Hirari"
            ));

            writeln!(socket).unwrap();
            let ack = ServerMessage::HandshakeAck {
                session_id: AppSessionId::new_v4(),
                server_version: "test".to_owned(),
            };
            writeln!(socket, "{}", serde_json::to_string(&ack).unwrap()).unwrap();

            let registration = read_client_message(&mut reader);
            observed_tx.send(registration.clone()).unwrap();
            assert!(matches!(
                registration,
                ClientMessage::RegisterEntity(EntityDescriptor { source_app, .. })
                    if source_app == "Hirari"
            ));

            let source_id = EntityId::from("moufu://source/companion");
            let link = LinkInfo {
                link_id: moufu_protocol::LinkId::new(),
                source_app: "Companion".to_owned(),
                source_entity: source_id.clone(),
                target_app: "Hirari".to_owned(),
                target_entity: EntityId::from(HIRARI_PROJECT_ENTITY),
                data_type: HIRARI_PROJECT_DATA_TYPE.to_owned(),
                status: LinkStatus::Active,
                last_synced_version: 0,
                created_at: chrono::Utc::now(),
            };
            let link_status = ServerMessage::LinkStatusChanged(link);
            writeln!(socket, "{}", serde_json::to_string(&link_status).unwrap()).unwrap();

            let mut received_pull = false;
            let mut received_live_snapshot = false;
            while !received_pull || !received_live_snapshot {
                let message = read_client_message(&mut reader);
                match message {
                    ClientMessage::PullEntity { entity_id } => {
                        assert_eq!(entity_id, source_id);
                        received_pull = true;
                    }
                    ClientMessage::NotifyChange(change) if change.is_transient => {
                        observed_tx
                            .send(ClientMessage::NotifyChange(change.clone()))
                            .unwrap();
                        assert!(matches!(
                            change.payload,
                            PayloadTransport::Inline(ref value)
                                if value["title"] == "Night Drive"
                                    && value["tracks"][0].get("path").is_none()
                                    && value["tracks"][0].get("frozen_path").is_none()
                                    && value["tracks"][0].get("plugin_state_hex").is_none()
                        ));
                        received_live_snapshot = true;
                    }
                    other => panic!("unexpected client message: {other:?}"),
                }
            }

            let state = ServerMessage::EntityState {
                entity_id: source_id.clone(),
                version: 42,
                payload: PayloadTransport::Inline(serde_json::json!({"title": "Companion"})),
            };
            writeln!(socket, "{}", serde_json::to_string(&state).unwrap()).unwrap();
            let update = ServerMessage::EntityUpdated(ChangeEvent {
                entity_id: source_id,
                version: 42,
                is_transient: true,
                timestamp: chrono::Utc::now(),
                payload: PayloadTransport::Inline(serde_json::json!({"title": "Companion"})),
                delta: None,
            });
            writeln!(socket, "{}", serde_json::to_string(&update).unwrap()).unwrap();

            let commit = read_client_message(&mut reader);
            observed_tx.send(commit.clone()).unwrap();
            assert!(matches!(
                commit,
                ClientMessage::NotifyChange(ChangeEvent {
                    is_transient: false,
                    payload: PayloadTransport::Inline(ref value),
                    ..
                }) if value["title"] == "Night Drive"
            ));
        });

        let publisher = MoufuPublisher::connect_to(&address.to_string()).unwrap();
        publisher.publish_layout(
            r#"{"title":"Night Drive","tracks":[{"name":"Vocal","path":"/Users/private/voice.wav","frozen_path":"/Users/private/frozen.wav","plugin_state_hex":["secret"],"volume":0.8}]}"#,
            true,
        );

        let handshake = observed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            handshake,
            ClientMessage::Handshake { app_name, .. } if app_name == "Hirari"
        ));
        let registration = observed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            registration,
            ClientMessage::RegisterEntity(EntityDescriptor {
                id: EntityId(ref id),
                data_type,
                ..
            }) if id == HIRARI_PROJECT_ENTITY && data_type == HIRARI_PROJECT_DATA_TYPE
        ));
        let mut received_connected = false;
        let mut received_update = false;
        let event_deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < event_deadline && !received_update {
            if let Some(event) = publisher.recv_event_timeout(Duration::from_millis(100)) {
                match event {
                    MoufuEvent::Connected { server_version } => {
                        received_connected = server_version == "test";
                    }
                    MoufuEvent::EntityUpdated(event) => {
                        received_update = event.version == 42
                            && event.entity_id == EntityId::from("moufu://source/companion")
                            && matches!(event.payload, PayloadTransport::Inline(ref payload) if payload["title"] == "Companion");
                    }
                    _ => {}
                }
            }
        }
        assert!(received_connected);
        assert!(received_update);
        let event = observed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            event,
            ClientMessage::NotifyChange(ChangeEvent {
                is_transient: true,
                payload: PayloadTransport::Inline(ref payload),
                ..
            }) if payload["title"] == "Night Drive"
                && payload["tracks"][0]["name"] == "Vocal"
                && payload["tracks"][0]["volume"] == 0.8
                && payload["tracks"][0].get("path").is_none()
                && payload["tracks"][0].get("frozen_path").is_none()
                && payload["tracks"][0].get("plugin_state_hex").is_none()
        ));
        publisher.publish_layout(
            r#"{"title":"Night Drive","tracks":[{"name":"Vocal","path":"/Users/private/voice.wav","frozen_path":"/Users/private/frozen.wav","plugin_state_hex":["secret"],"volume":0.8}]}"#,
            false,
        );
        let committed = observed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            committed,
            ClientMessage::NotifyChange(ChangeEvent {
                is_transient: false,
                ..
            })
        ));

        publisher.shutdown();
        server.join().unwrap();
    }
}
