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

#[path = "moufu_wire.rs"]
mod wire;
pub(crate) use wire::*;

#[path = "moufu_worker.rs"]
mod worker;
pub(crate) use worker::*;

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

/// How many times a single project snapshot version is retried after a failed
/// publish before the adapter gives up on it and moves on to newer versions.
const MAX_PUBLISH_ATTEMPTS: u32 = 5;
/// First retry delay after a failed publish; doubles up to `PUBLISH_BACKOFF_MAX`.
const PUBLISH_BACKOFF_BASE: Duration = Duration::from_millis(250);
/// Upper bound on the retry delay so a recovered project republishes promptly.
const PUBLISH_BACKOFF_MAX: Duration = Duration::from_secs(5);

/// Outcome of one publish attempt, so the caller can keep the version marker
/// in place for anything that was not actually written to the socket.
enum PublishOutcome {
    Sent,
    Rejected(String),
}

/// Record a failed attempt and schedule the next retry with exponential backoff.
fn defer_publish(attempts: &mut u32, next_publish_at: &mut Instant, now: Instant) {
    *attempts = attempts.saturating_add(1);
    let shift = (*attempts).saturating_sub(1).min(8);
    let delay = PUBLISH_BACKOFF_BASE
        .saturating_mul(1u32 << shift)
        .min(PUBLISH_BACKOFF_MAX);
    *next_publish_at = now + delay;
}

fn reset_publish_backoff(attempts: &mut u32, next_publish_at: &mut Instant) {
    *attempts = 0;
    *next_publish_at = Instant::now();
}

fn write_change_event(
    writer: &mut TcpStream,
    events: &MoufuEventQueue,
    generation: u64,
    event: ChangeEvent,
) -> io::Result<PublishOutcome> {
    let mut message = BoundedMessageBuffer {
        bytes: Vec::with_capacity(64 * 1024),
        limit: MAX_CLIENT_MESSAGE_BYTES.saturating_sub(1),
        exceeded: false,
    };
    match serde_json::to_writer(&mut message, &ClientMessage::NotifyChange(event)) {
        Ok(()) => {
            message.bytes.push(b'\n');
            writer.write_all(&message.bytes)?;
            writer.flush()?;
            Ok(PublishOutcome::Sent)
        }
        Err(_) if message.exceeded => {
            let reason = format!(
                "Project snapshot exceeds Moufu's {} MiB message limit",
                MAX_CLIENT_MESSAGE_BYTES / (1024 * 1024)
            );
            log::error!("Hirari {reason} and was not sent");
            publish_event(
                events,
                generation,
                MoufuEvent::ServerNotice {
                    code: "PROJECT_SNAPSHOT_TOO_LARGE".to_owned(),
                    message: format!("{reason} and was not sent"),
                },
            );
            Ok(PublishOutcome::Rejected(reason))
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
#[path = "lib_tests.rs"]
mod tests;
