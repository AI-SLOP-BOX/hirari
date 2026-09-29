use std::collections::VecDeque;
use std::ffi::c_void;

const MAX_HISTORY: usize = 16;
const MAX_SNAPSHOT_SAMPLES: usize = 16 * 1024 * 1024;
const MAX_HISTORY_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone)]
struct Snapshot {
    id: u64,
    owner: usize,
    channels: u32,
    frames: u32,
    label: String,
    data: Vec<f32>,
}

struct RestoreToken {
    owner: usize,
    id: u64,
    redo: bool,
}

#[derive(Default)]
struct SpectralHistory {
    next_id: u64,
    undo: VecDeque<Snapshot>,
    redo: VecDeque<Snapshot>,
}

impl SpectralHistory {
    fn bytes(&self) -> usize {
        self.undo
            .iter()
            .chain(&self.redo)
            .map(|entry| entry.data.len().saturating_mul(std::mem::size_of::<f32>()))
            .fold(0usize, usize::saturating_add)
    }

    fn trim(&mut self) {
        while self.bytes() > MAX_HISTORY_BYTES && (!self.undo.is_empty() || !self.redo.is_empty()) {
            if !self.undo.is_empty() {
                self.undo.pop_front();
            } else {
                self.redo.pop_front();
            }
        }
    }

    fn capture(&mut self, mut snapshot: Snapshot) {
        snapshot.id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        if self.undo.len() >= MAX_HISTORY {
            self.undo.pop_front();
        }
        self.undo.push_back(snapshot);
        self.redo.clear();
        self.trim();
    }

    fn entry(&self, owner: usize, redo: bool) -> Option<&Snapshot> {
        let history = if redo { &self.redo } else { &self.undo };
        if owner == 0 {
            history.back()
        } else {
            history.iter().rev().find(|entry| entry.owner == owner)
        }
    }
}

unsafe fn make_snapshot(
    owner: usize,
    channels: u32,
    frames: u32,
    data: *const *const f32,
    label: *const u8,
    label_len: usize,
) -> Option<Snapshot> {
    let total = (channels as usize).checked_mul(frames as usize)?;
    if total > MAX_SNAPSHOT_SAMPLES || (channels > 0 && data.is_null()) {
        return None;
    }
    let mut samples = Vec::new();
    samples.try_reserve_exact(total).ok()?;
    let pointers = if channels > 0 {
        Some(std::slice::from_raw_parts(data, channels as usize))
    } else {
        None
    };
    for channel in 0..channels as usize {
        let pointer = *pointers.as_ref()?.get(channel)?;
        if pointer.is_null() {
            return None;
        }
        samples.extend_from_slice(std::slice::from_raw_parts(pointer, frames as usize));
    }
    let label = if label_len == 0 {
        String::new()
    } else {
        if label.is_null() {
            return None;
        }
        String::from_utf8_lossy(std::slice::from_raw_parts(label, label_len)).into_owned()
    };
    Some(Snapshot {
        id: 0,
        owner,
        channels,
        frames,
        label,
        data: samples,
    })
}

#[no_mangle]
pub extern "C" fn hirari_spectral_history_create() -> *mut c_void {
    Box::into_raw(Box::new(SpectralHistory::default())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<SpectralHistory>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_capture(
    state: *mut c_void,
    owner: usize,
    channels: u32,
    frames: u32,
    data: *const *const f32,
    label: *const u8,
    label_len: usize,
) -> bool {
    if state.is_null() {
        return false;
    }
    let Some(snapshot) = make_snapshot(owner, channels, frames, data, label, label_len) else {
        return false;
    };
    (*state.cast::<SpectralHistory>()).capture(snapshot);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_snapshot_create(
    owner: usize,
    channels: u32,
    frames: u32,
    data: *const *const f32,
    label: *const u8,
    label_len: usize,
) -> *mut c_void {
    make_snapshot(owner, channels, frames, data, label, label_len)
        .map_or(std::ptr::null_mut(), |snapshot| {
            Box::into_raw(Box::new(snapshot)).cast()
        })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_snapshot_destroy(snapshot: *mut c_void) {
    if !snapshot.is_null() {
        drop(Box::from_raw(snapshot.cast::<Snapshot>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_peek(
    state: *const c_void,
    owner: usize,
    redo: bool,
) -> *mut c_void {
    if state.is_null() {
        return std::ptr::null_mut();
    }
    (*state.cast::<SpectralHistory>())
        .entry(owner, redo)
        .map_or(std::ptr::null_mut(), |snapshot| {
            Box::into_raw(Box::new(RestoreToken {
                owner,
                id: snapshot.id,
                redo,
            }))
            .cast()
        })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_token_destroy(token: *mut c_void) {
    if !token.is_null() {
        drop(Box::from_raw(token.cast::<RestoreToken>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_label(
    state: *const c_void,
    owner: usize,
    redo: bool,
    output: *mut u8,
    capacity: usize,
) -> usize {
    if state.is_null() {
        return 0;
    }
    let Some(snapshot) = (*state.cast::<SpectralHistory>()).entry(owner, redo) else {
        return 0;
    };
    let label = snapshot.label.as_bytes();
    if !output.is_null() && capacity > 0 {
        std::ptr::copy_nonoverlapping(label.as_ptr(), output, label.len().min(capacity));
    }
    label.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_snapshot_channels(
    state: *const c_void,
    token: *const c_void,
) -> u32 {
    if state.is_null() || token.is_null() {
        return 0;
    }
    let token = &*token.cast::<RestoreToken>();
    (*state.cast::<SpectralHistory>())
        .entry(token.owner, token.redo)
        .filter(|entry| entry.id == token.id)
        .map_or(0, |entry| entry.channels)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_snapshot_frames(
    state: *const c_void,
    token: *const c_void,
) -> u32 {
    if state.is_null() || token.is_null() {
        return 0;
    }
    let token = &*token.cast::<RestoreToken>();
    (*state.cast::<SpectralHistory>())
        .entry(token.owner, token.redo)
        .filter(|entry| entry.id == token.id)
        .map_or(0, |entry| entry.frames)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_snapshot_label(
    state: *const c_void,
    token: *const c_void,
    output: *mut u8,
    capacity: usize,
) -> usize {
    if state.is_null() || token.is_null() {
        return 0;
    }
    let token = &*token.cast::<RestoreToken>();
    let Some(snapshot) = (*state.cast::<SpectralHistory>())
        .entry(token.owner, token.redo)
        .filter(|entry| entry.id == token.id)
    else {
        return 0;
    };
    let label = snapshot.label.as_bytes();
    if !output.is_null() && capacity > 0 {
        std::ptr::copy_nonoverlapping(label.as_ptr(), output, label.len().min(capacity));
    }
    label.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_snapshot_restore(
    state: *const c_void,
    token: *const c_void,
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
) -> bool {
    if state.is_null() || token.is_null() || channels.is_null() {
        return false;
    }
    let token = &*token.cast::<RestoreToken>();
    let Some(snapshot) = (*state.cast::<SpectralHistory>())
        .entry(token.owner, token.redo)
        .filter(|entry| entry.id == token.id)
    else {
        return false;
    };
    if snapshot.channels == 0
        || snapshot.frames == 0
        || channel_count != snapshot.channels
        || frames != snapshot.frames
        || snapshot.data.len() != channel_count as usize * frames as usize
    {
        return false;
    }
    let pointers = std::slice::from_raw_parts(channels, channel_count as usize);
    if pointers.iter().any(|pointer| pointer.is_null()) {
        return false;
    }
    for channel in 0..channel_count as usize {
        std::ptr::copy_nonoverlapping(
            snapshot.data.as_ptr().add(channel * frames as usize),
            pointers[channel],
            frames as usize,
        );
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_finish_restore(
    state: *mut c_void,
    owner: usize,
    redo: bool,
    target: *mut c_void,
    current: *mut c_void,
) -> bool {
    if state.is_null() || target.is_null() || current.is_null() {
        return false;
    }
    let token = &*target.cast::<RestoreToken>();
    if token.owner != owner || token.redo != redo {
        return false;
    }
    let history = &mut *state.cast::<SpectralHistory>();
    let selected = if redo {
        &mut history.redo
    } else {
        &mut history.undo
    };
    let Some(index) = selected
        .iter()
        .rposition(|entry| entry.owner == owner && entry.id == token.id)
    else {
        return false;
    };
    let mut current = Box::from_raw(current.cast::<Snapshot>());
    current.id = history.next_id;
    current.owner = owner;
    history.next_id = history.next_id.wrapping_add(1).max(1);
    selected.remove(index);
    let destination = if redo {
        &mut history.undo
    } else {
        &mut history.redo
    };
    destination.push_back(*current);
    drop(Box::from_raw(target.cast::<RestoreToken>()));
    history.trim();
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_contains(
    state: *const c_void,
    owner: usize,
    redo: bool,
) -> bool {
    !state.is_null()
        && (*state.cast::<SpectralHistory>())
            .entry(owner, redo)
            .is_some()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_depth(
    state: *const c_void,
    owner: usize,
    redo: bool,
) -> usize {
    if state.is_null() {
        return 0;
    }
    let history = if redo {
        &(*state.cast::<SpectralHistory>()).redo
    } else {
        &(*state.cast::<SpectralHistory>()).undo
    };
    if owner == 0 {
        history.len()
    } else {
        history.iter().filter(|entry| entry.owner == owner).count()
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_bytes(state: *const c_void) -> usize {
    if state.is_null() {
        0
    } else {
        (*state.cast::<SpectralHistory>()).bytes()
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_history_clear(state: *mut c_void, owner: usize) {
    if state.is_null() {
        return;
    }
    let history = &mut *state.cast::<SpectralHistory>();
    if owner == 0 {
        history.undo.clear();
        history.redo.clear();
    } else {
        history.undo.retain(|entry| entry.owner != owner);
        history.redo.retain(|entry| entry.owner != owner);
    }
}
