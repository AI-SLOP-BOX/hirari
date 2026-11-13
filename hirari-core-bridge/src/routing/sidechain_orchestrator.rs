//! Sidechain orchestrator front door: tap points, link shapes and the
//! routing control surface that the UI/DSP layer talks to.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidechainTapPointRust {
    PreFX,
    PostFX,
    PostFader,
}



#[derive(Clone, Debug)]
pub struct SidechainLinkRust {
    pub source_track_id: u32,
    pub dest_track_id: u32,
    pub plugin_idx: u32,
    /// Zero-based side-chain input exposed by the plug-in.
    pub input_bus: u16,
    pub enabled: bool,
    pub level: f32,
    pub tap_point: SidechainTapPointRust,
}

pub struct SidechainOrchestrator {
    pub active_links: HashMap<u64, SidechainLinkRust>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SidechainLinkError {
    EmptyId,
    SelfReference,
    DuplicateId,
    DuplicateRoute,
    InvalidParameters,
    CircularReference,
}

impl Default for SidechainOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SidechainOrchestrator {
    pub fn new() -> Self {
        Self {
            active_links: HashMap::new(),
        }
    }

    /// Registers a link after checking that it can be added to the routing graph.
    pub fn register_link(
        &mut self,
        link_id: u64,
        link: SidechainLinkRust,
    ) -> Result<(), SidechainLinkError> {
        if link_id == 0 || link.source_track_id == 0 || link.dest_track_id == 0 {
            return Err(SidechainLinkError::EmptyId);
        }
        if link.source_track_id == link.dest_track_id {
            return Err(SidechainLinkError::SelfReference);
        }
        if self.active_links.contains_key(&link_id) {
            return Err(SidechainLinkError::DuplicateId);
        }
        if link.input_bus >= 256 || !link.level.is_finite() || !(0.0..=1.0).contains(&link.level) {
            return Err(SidechainLinkError::InvalidParameters);
        }
        if self.active_links.values().any(|existing| {
            existing.source_track_id == link.source_track_id
                && existing.dest_track_id == link.dest_track_id
                && existing.plugin_idx == link.plugin_idx
                && existing.input_bus == link.input_bus
        }) {
            return Err(SidechainLinkError::DuplicateRoute);
        }
        if self.would_create_cycle(link.source_track_id, link.dest_track_id) {
            return Err(SidechainLinkError::CircularReference);
        }

        self.active_links.insert(link_id, link);
        Ok(())
    }

    /// Compatibility convenience for callers that only need success/failure.
    pub fn add_sidechain_link(&mut self, link_id: u64, link: SidechainLinkRust) -> bool {
        self.register_link(link_id, link).is_ok()
    }

    /// Resolves a link ID without panicking for an unknown ID.
    pub fn resolve_link(&self, link_id: u64) -> Option<&SidechainLinkRust> {
        self.active_links.get(&link_id)
    }

    pub fn remove_link(&mut self, link_id: u64) -> bool {
        self.active_links.remove(&link_id).is_some()
    }

    /// Removes every side-chain endpoint owned by a plugin instance. This is
    /// used when a plugin is deleted or bypassed so stale dynamic ports cannot
    /// remain in the routing graph.
    pub fn remove_plugin_links(&mut self, dest_track_id: u32, plugin_idx: u32) -> usize {
        let before = self.active_links.len();
        self.active_links.retain(|_, link| {
            !(link.dest_track_id == dest_track_id && link.plugin_idx == plugin_idx)
        });
        before - self.active_links.len()
    }

    /// Returns all sources feeding a plugin in deterministic link-ID order.
    pub fn links_for_destination(
        &self,
        dest_track_id: u32,
        plugin_idx: u32,
    ) -> Vec<(u64, u32, u16, bool, f32, SidechainTapPointRust)> {
        let mut links: Vec<_> = self
            .active_links
            .iter()
            .filter(|(_, link)| {
                link.dest_track_id == dest_track_id && link.plugin_idx == plugin_idx
            })
            .map(|(id, link)| {
                (
                    *id,
                    link.source_track_id,
                    link.input_bus,
                    link.enabled,
                    link.level,
                    link.tap_point,
                )
            })
            .collect();
        links.sort_by_key(|entry| entry.0);
        links
    }

    pub fn sources_for_input(
        &self,
        dest_track_id: u32,
        plugin_idx: u32,
        input_bus: u16,
    ) -> Vec<(u64, u32, f32)> {
        let mut sources: Vec<_> = self
            .active_links
            .iter()
            .filter(|(_, link)| {
                link.enabled
                    && link.dest_track_id == dest_track_id
                    && link.plugin_idx == plugin_idx
                    && link.input_bus == input_bus
            })
            .map(|(id, link)| (*id, link.source_track_id, link.level))
            .collect();
        sources.sort_by_key(|entry| entry.0);
        sources
    }

    pub fn set_enabled(&mut self, link_id: u64, enabled: bool) -> bool {
        let Some(link) = self.active_links.get_mut(&link_id) else {
            return false;
        };
        link.enabled = enabled;
        true
    }

    pub fn update_level(&mut self, link_id: u64, level: f32) -> bool {
        if !level.is_finite() || !(0.0..=1.0).contains(&level) {
            return false;
        }
        let Some(link) = self.active_links.get_mut(&link_id) else {
            return false;
        };
        link.level = level;
        true
    }

    pub fn set_tap_point(&mut self, link_id: u64, tap_point: SidechainTapPointRust) -> bool {
        let Some(link) = self.active_links.get_mut(&link_id) else {
            return false;
        };
        link.tap_point = tap_point;
        true
    }

    /// Resolves the source track for a destination/plugin pair.
    pub fn resolve_source_for(&self, dest_track_id: u32, plugin_idx: u32) -> Option<u32> {
        if dest_track_id == 0 {
            return None;
        }
        self.active_links
            .iter()
            .filter(|(_, link)| {
                link.enabled && link.dest_track_id == dest_track_id && link.plugin_idx == plugin_idx
            })
            .min_by_key(|(id, _)| *id)
            .map(|(_, link)| link.source_track_id)
    }

    /// Validates the already registered graph. Registration keeps this graph valid;
    /// this method remains the orchestration entry point for existing callers.
    pub fn resolve_sidechain_links(&mut self) {
        self.active_links.retain(|_, link| {
            link.source_track_id != 0
                && link.dest_track_id != 0
                && link.source_track_id != link.dest_track_id
        });
    }

    fn would_create_cycle(&self, source: u32, destination: u32) -> bool {
        let mut current = destination;
        let mut visited = std::collections::HashSet::new();
        while visited.insert(current) {
            if current == source {
                return true;
            }
            let Some(next) = self
                .active_links
                .values()
                .find(|link| link.source_track_id == current)
                .map(|link| link.dest_track_id)
            else {
                return false;
            };
            current = next;
        }
        false
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide routing state.
    pub fn audit_sidechain_manager(&self) -> bool {
        if self.active_links.len() > 65_536 {
            return false;
        }
        let mut routes = std::collections::HashSet::new();
        for (link_id, link) in &self.active_links {
            if *link_id == 0
                || link.source_track_id == 0
                || link.dest_track_id == 0
                || link.source_track_id == link.dest_track_id
                || !link.level.is_finite()
                || !(0.0..=1.0).contains(&link.level)
                || link.input_bus >= 256
                || !matches!(
                    link.tap_point,
                    SidechainTapPointRust::PreFX
                        | SidechainTapPointRust::PostFX
                        | SidechainTapPointRust::PostFader
                )
            {
                return false;
            }
            if !routes.insert((
                link.source_track_id,
                link.dest_track_id,
                link.plugin_idx,
                link.input_bus,
            )) {
                return false;
            }
        }

        // Validate the complete graph, not only the edge most recently added.
        // This catches imported/legacy snapshots that bypass register_link().
        for link in self.active_links.values() {
            if self.would_create_cycle_from_snapshot(link.source_track_id, link.dest_track_id) {
                return false;
            }
        }
        true
    }

    fn would_create_cycle_from_snapshot(&self, source: u32, destination: u32) -> bool {
        let mut current = destination;
        let mut visited = std::collections::HashSet::new();
        while visited.insert(current) {
            if current == source {
                return true;
            }
            let Some(next) = self
                .active_links
                .values()
                .filter(|link| link.source_track_id == current)
                .map(|link| link.dest_track_id)
                .next()
            else {
                return false;
            };
            current = next;
        }
        true
    }
}

// ---- Engine handle: gate + writer lock ownership lives in Rust ----------
//
// The C++ adapter used to carry the publication gate, a writer mutex and the
// begin/end control calls around every mutation. This handle owns all three so
// each exported call is a single FFI entry point.

struct SidechainEngine {
    gate: *mut c_void,
    writer: std::sync::Mutex<()>,
    legacy_misuse: AtomicBool,
}

unsafe impl Send for SidechainEngine {}

unsafe fn engine_ref<'a>(state: *const c_void) -> Option<&'a SidechainEngine> {
    // SAFETY: forwarded from each public engine function's live handle.
    unsafe { state.cast::<SidechainEngine>().as_ref() }
}

fn control_lock(engine: &SidechainEngine) -> std::sync::MutexGuard<'_, ()> {
    engine
        .writer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

unsafe fn with_control<T>(engine: &SidechainEngine, body: impl FnOnce(*const c_void) -> T) -> T {
    let _guard = control_lock(engine);
    unsafe { hirari_sidechain_publication_gate_begin_control(engine.gate) };
    let result = body(engine.gate);
    unsafe { hirari_sidechain_publication_gate_end_control(engine.gate) };
    result
}

#[no_mangle]
pub extern "C" fn hirari_sidechain_engine_create() -> *mut c_void {
    Box::into_raw(Box::new(SidechainEngine {
        gate: hirari_sidechain_publication_gate_create(),
        writer: std::sync::Mutex::new(()),
        legacy_misuse: AtomicBool::new(false),
    }))
    .cast()
}

// Process-wide fallback for native callers that build an effect chain without
// an engine handle. It is created once and intentionally never freed; the
// engine-owned handles cover the real session.
/// Legacy entry point kept so the retired in-place registration API still
/// reports its misuse exactly once per reset.
#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_resolve_links(state: *const c_void) -> bool {
    if let Some(engine) = unsafe { engine_ref(state) } {
        engine.legacy_misuse.store(true, Ordering::Release);
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_take_legacy_misuse(state: *const c_void) -> bool {
    match unsafe { engine_ref(state) } {
        Some(engine) => engine.legacy_misuse.swap(false, Ordering::AcqRel),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    // SAFETY: caller hands back a handle from hirari_sidechain_engine_create.
    let engine = unsafe { Box::from_raw(state.cast::<SidechainEngine>()) };
    unsafe { hirari_sidechain_publication_gate_destroy(engine.gate) };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_register_link(
    state: *const c_void,
    destination_track: u32,
    plugin_index: u32,
    source_track: u32,
    source_left: *const f32,
    source_right: *const f32,
    frames: u32,
    sample_rate: u32,
    source_generation: u64,
    level: f32,
    tap_point: u8,
) -> bool {
    let Some(engine) = (unsafe { engine_ref(state) }) else {
        return false;
    };
    unsafe {
        with_control(engine, |gate| unsafe {
            hirari_sidechain_register_link(
                gate,
                destination_track,
                plugin_index,
                source_track,
                source_left,
                source_right,
                frames,
                sample_rate,
                source_generation,
                level,
                tap_point,
            )
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_register_silent_link(
    state: *const c_void,
    destination_track: u32,
    plugin_index: u32,
    source_track: u32,
    frames: u32,
    sample_rate: u32,
    source_generation: u64,
    level: f32,
    tap_point: u8,
) -> bool {
    let Some(engine) = (unsafe { engine_ref(state) }) else {
        return false;
    };
    unsafe {
        with_control(engine, |gate| unsafe {
            hirari_sidechain_register_silent_link(
                gate,
                destination_track,
                plugin_index,
                source_track,
                frames,
                sample_rate,
                source_generation,
                level,
                tap_point,
            )
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_remove_link(
    state: *const c_void,
    destination_track: u32,
    plugin_index: u32,
) -> bool {
    let Some(engine) = (unsafe { engine_ref(state) }) else {
        return false;
    };
    unsafe {
        with_control(engine, |gate| unsafe {
            hirari_sidechain_remove_link(gate, destination_track, plugin_index)
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_has_link(
    state: *const c_void,
    destination_track: u32,
    plugin_index: u32,
    source_track: u32,
) -> bool {
    let Some(engine) = (unsafe { engine_ref(state) }) else {
        return false;
    };
    unsafe { hirari_sidechain_has_link(engine.gate, destination_track, plugin_index, source_track) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_remove_track(state: *const c_void, track: u32) {
    let Some(engine) = (unsafe { engine_ref(state) }) else {
        return;
    };
    unsafe {
        with_control(engine, |gate| unsafe {
            hirari_sidechain_remove_track(gate, track)
        })
    };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_reset(state: *const c_void) {
    let Some(engine) = (unsafe { engine_ref(state) }) else {
        return;
    };
    unsafe {
        with_control(engine, |gate| unsafe { hirari_sidechain_reset(gate) })
    };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_copy_link(
    state: *const c_void,
    destination_track: u32,
    plugin_index: u32,
    output_left: *mut f32,
    output_right: *mut f32,
    capacity: u32,
    metadata: *mut u64,
) -> u32 {
    let Some(engine) = (unsafe { engine_ref(state) }) else {
        return 0;
    };
    if !unsafe { hirari_sidechain_publication_gate_enter_reader(engine.gate) } {
        return 0;
    }
    let frames = unsafe {
        hirari_sidechain_copy_link(
            engine.gate,
            destination_track,
            plugin_index,
            output_left,
            output_right,
            capacity,
            metadata,
        )
    };
    unsafe { hirari_sidechain_publication_gate_leave_reader(engine.gate) };
    frames
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_refresh_source(
    state: *const c_void,
    source_track: u32,
    left: *const f32,
    right: *const f32,
    frames: u32,
    sample_rate: u32,
    source_generation: u64,
) -> bool {
    let Some(engine) = (unsafe { engine_ref(state) }) else {
        return false;
    };
    unsafe {
        with_control(engine, |gate| unsafe {
            hirari_sidechain_refresh_source(
                gate,
                source_track,
                left,
                right,
                frames,
                sample_rate,
                source_generation,
            )
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_publish_source(
    state: *const c_void,
    source_track: u32,
    pre_left: *const f32,
    pre_right: *const f32,
    post_left: *const f32,
    post_right: *const f32,
    fader_left: *const f32,
    fader_right: *const f32,
    frames: u32,
    sample_rate: u32,
    source_generation: u64,
) {
    let Some(engine) = (unsafe { engine_ref(state) }) else {
        return;
    };
    if !unsafe { hirari_sidechain_publication_gate_try_begin_audio(engine.gate) } {
        return;
    }
    unsafe {
        hirari_sidechain_publish_source(
            engine.gate,
            source_track,
            pre_left,
            pre_right,
            post_left,
            post_right,
            fader_left,
            fader_right,
            frames,
            sample_rate,
            source_generation,
        )
    };
    unsafe { hirari_sidechain_publication_gate_end_audio(engine.gate) };
}

// ---- Captured links: the undo snapshot for a removed track ---------------
//
// Detaching a track has to remember its sidechain links so reinserting it can
// restore them. The snapshot is owned here; the engine only holds the handle.

struct CapturedLink {
    destination: u32,
    plugin_index: u32,
    source: u32,
    level: f32,
    tap: u8,
    sample_rate: u32,
    generation: u64,
    left: Vec<f32>,
    right: Vec<f32>,
}

struct CapturedLinks(Vec<CapturedLink>);

unsafe extern "C" fn capture_links_callback(
    context: *mut c_void,
    destination: u32,
    plugin_index: u32,
    source: u32,
    level: f32,
    tap: u8,
    sample_rate: u32,
    generation: u64,
    left: *const f32,
    right: *const f32,
    frames: u32,
) -> bool {
    if context.is_null() || left.is_null() || right.is_null() || frames == 0 {
        return false;
    }
    // SAFETY: the context is the Vec this call is collecting into, and the
    // gate keeps the published buffers alive for the duration of the walk.
    let links = unsafe { &mut *context.cast::<Vec<CapturedLink>>() };
    let count = frames as usize;
    links.push(CapturedLink {
        destination,
        plugin_index,
        source,
        level,
        tap,
        sample_rate,
        generation,
        left: unsafe { std::slice::from_raw_parts(left, count) }.to_vec(),
        right: unsafe { std::slice::from_raw_parts(right, count) }.to_vec(),
    });
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_capture_links(
    state: *const c_void,
    track: u32,
) -> *mut c_void {
    let Some(engine) = (unsafe { engine_ref(state) }) else {
        return std::ptr::null_mut();
    };
    if track == 0 {
        return std::ptr::null_mut();
    }
    let mut links: Vec<CapturedLink> = Vec::new();
    let captured = unsafe {
        with_control(engine, |gate| unsafe {
            hirari_sidechain_capture_track(
                gate,
                track,
                (&mut links as *mut Vec<CapturedLink>).cast(),
                Some(capture_links_callback),
            )
        })
    };
    if !captured {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(CapturedLinks(links))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_restore_links(
    state: *const c_void,
    handle: *const c_void,
) -> bool {
    if state.is_null() || handle.is_null() {
        return false;
    }
    // SAFETY: handle comes from hirari_sidechain_engine_capture_links and is
    // only read here until it is handed back for freeing.
    let links = unsafe { &*handle.cast::<CapturedLinks>() };
    let mut all_registered = true;
    for link in &links.0 {
        // Same admission the retired adapter applied before re-registering:
        // an unowned or malformed capture is skipped, not propagated.
        if link.destination == 0
            || link.source == 0
            || link.left.is_empty()
            || link.left.len() != link.right.len()
            || link.left.len() > SIDECHAIN_BLOCK
            || link.sample_rate == 0
            || link.generation == 0
        {
            continue;
        }
        let registered = unsafe {
            hirari_sidechain_engine_register_link(
                state,
                link.destination,
                link.plugin_index,
                link.source,
                link.left.as_ptr(),
                link.right.as_ptr(),
                link.left.len() as u32,
                link.sample_rate,
                link.generation,
                link.level,
                link.tap,
            )
        };
        all_registered &= registered;
    }
    all_registered
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_engine_free_captured_links(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    // SAFETY: handle is exclusively owned by the caller at this point.
    drop(unsafe { Box::from_raw(handle.cast::<CapturedLinks>()) });
}
