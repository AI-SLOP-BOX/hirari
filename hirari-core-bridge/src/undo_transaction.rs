use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

type NativeCallbackFn = unsafe extern "C" fn(*mut c_void);

struct NativeCallback {
    context: *mut c_void,
    invoke: NativeCallbackFn,
    destroy: NativeCallbackFn,
}

unsafe impl Send for NativeCallback {}

impl Drop for NativeCallback {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.context) };
    }
}

#[derive(Clone)]
struct ActionRecord {
    name: String,
    undo_callback: u64,
    redo_callback: u64,
    timestamp_ms: u64,
}

enum HistoryEntry {
    Action(ActionRecord),
    Transaction { name: String, actions: Vec<ActionRecord> },
}

impl HistoryEntry {
    fn name(&self) -> &str {
        match self {
            Self::Action(action) => &action.name,
            Self::Transaction { name, .. } => name,
        }
    }

    fn callback_ids(&self, redo: bool, output: &mut Vec<u64>) {
        match self {
            Self::Action(action) => {
                output.push(if redo { action.redo_callback } else { action.undo_callback });
            }
            Self::Transaction { actions, .. } => {
                if redo {
                    output.extend(actions.iter().map(|action| action.redo_callback));
                } else {
                    output.extend(actions.iter().rev().map(|action| action.undo_callback));
                }
            }
        }
    }

    fn visit_callback_ids(&self, output: &mut Vec<u64>) {
        match self {
            Self::Action(action) => {
                output.push(action.undo_callback);
                output.push(action.redo_callback);
            }
            Self::Transaction { actions, .. } => {
                for action in actions {
                    output.push(action.undo_callback);
                    output.push(action.redo_callback);
                }
            }
        }
    }
}

struct UndoHistoryState {
    undo: VecDeque<HistoryEntry>,
    redo: VecDeque<HistoryEntry>,
    transaction: Option<(String, Vec<ActionRecord>)>,
    max_depth: usize,
    coalesce_window_ms: u64,
    native_callbacks: HashMap<u64, Arc<NativeCallback>>,
    pending_callbacks: HashSet<u64>,
    next_callback_id: u64,
}

impl UndoHistoryState {
    fn new(max_depth: usize, coalesce_window_ms: u64) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            transaction: None,
            max_depth: max_depth.max(1),
            coalesce_window_ms,
            native_callbacks: HashMap::new(),
            pending_callbacks: HashSet::new(),
            next_callback_id: 1,
        }
    }

    fn register_callback(
        &mut self,
        context: *mut c_void,
        invoke: NativeCallbackFn,
        destroy: NativeCallbackFn,
    ) -> u64 {
        loop {
            let id = self.next_callback_id;
            self.next_callback_id = self.next_callback_id.wrapping_add(1);
            if id != 0 && !self.native_callbacks.contains_key(&id) {
                self.native_callbacks.insert(
                    id,
                    Arc::new(NativeCallback { context, invoke, destroy }),
                );
                self.pending_callbacks.insert(id);
                return id;
            }
        }
    }

    fn record(&mut self, action: ActionRecord, coalesce: bool) {
        if let Some((_, actions)) = &mut self.transaction {
            actions.push(action);
            return;
        }

        if coalesce {
            if let Some(HistoryEntry::Action(previous)) = self.undo.back_mut() {
                if previous.name == action.name
                    && action.timestamp_ms >= previous.timestamp_ms
                    && action.timestamp_ms - previous.timestamp_ms <= self.coalesce_window_ms
                {
                    previous.redo_callback = action.redo_callback;
                    previous.timestamp_ms = action.timestamp_ms;
                    self.redo.clear();
                    return;
                }
            }
        }

        self.undo.push_back(HistoryEntry::Action(action));
        self.trim_undo();
        self.redo.clear();
    }

    fn trim_undo(&mut self) {
        while self.undo.len() > self.max_depth {
            self.undo.pop_front();
        }
    }

    fn begin_transaction(&mut self, name: String) {
        self.transaction = Some((
            if name.is_empty() { "transaction".to_owned() } else { name },
            Vec::new(),
        ));
    }

    fn end_transaction(&mut self) -> bool {
        let Some((name, actions)) = self.transaction.take() else {
            return false;
        };
        if actions.is_empty() {
            return true;
        }
        self.undo.push_back(HistoryEntry::Transaction { name, actions });
        self.trim_undo();
        self.redo.clear();
        true
    }

    fn next_callback_count(&self, redo: bool) -> usize {
        let stack = if redo { &self.redo } else { &self.undo };
        let Some(entry) = stack.back() else { return 0; };
        match entry {
            HistoryEntry::Action(_) => 1,
            HistoryEntry::Transaction { actions, .. } => actions.len(),
        }
    }

    fn apply_history(&mut self, redo: bool, output: &mut [u64]) -> usize {
        let required = self.next_callback_count(redo);
        if required == 0 || output.len() < required {
            return required;
        }
        let (source, destination) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        let Some(entry) = source.pop_back() else { return 0; };
        let mut callback_ids = Vec::with_capacity(required);
        entry.callback_ids(redo, &mut callback_ids);
        output[..required].copy_from_slice(&callback_ids);
        destination.push_back(entry);
        required
    }

    fn abort_transaction(&mut self, output: &mut [u64]) -> Option<usize> {
        let required = self.transaction.as_ref()?.1.len();
        if output.len() < required {
            return Some(required);
        }
        let (_, actions) = self.transaction.take()?;
        for (output_id, action) in output.iter_mut().zip(actions.iter().rev()) {
            *output_id = action.undo_callback;
        }
        Some(required)
    }

    fn live_callback_ids(&self) -> Vec<u64> {
        let mut output = Vec::new();
        for entry in self.undo.iter().chain(self.redo.iter()) {
            entry.visit_callback_ids(&mut output);
        }
        if let Some((_, actions)) = &self.transaction {
            for action in actions {
                output.push(action.undo_callback);
                output.push(action.redo_callback);
            }
        }
        output.sort_unstable();
        output.dedup();
        output
    }

    fn cleanup_native_callbacks(&mut self) {
        let mut retained = self.live_callback_ids().into_iter().collect::<HashSet<_>>();
        retained.extend(self.pending_callbacks.iter().copied());
        self.native_callbacks.retain(|id, _| retained.contains(id));
    }
}

fn invoke_native_callbacks(state: &Mutex<UndoHistoryState>, ids: &[u64]) -> bool {
    let callbacks = {
        let state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ids.iter()
            .map(|id| state.native_callbacks.get(id).cloned())
            .collect::<Option<Vec<_>>>()
    };
    let Some(callbacks) = callbacks else { return false; };
    for callback in callbacks {
        unsafe { (callback.invoke)(callback.context) };
    }
    true
}

fn callbacks_for(state: &UndoHistoryState, ids: &[u64]) -> Option<Vec<Arc<NativeCallback>>> {
    ids.iter()
        .map(|id| state.native_callbacks.get(id).cloned())
        .collect()
}

fn invoke_callbacks(callbacks: Vec<Arc<NativeCallback>>) {
    for callback in callbacks {
        unsafe { (callback.invoke)(callback.context) };
    }
}

static CLOCK_ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

#[no_mangle]
pub extern "C" fn hirari_undo_manager_timestamp_ms() -> u64 {
    let origin = CLOCK_ORIGIN.get_or_init(Instant::now);
    Instant::now()
        .saturating_duration_since(*origin)
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[no_mangle]
pub extern "C" fn hirari_undo_manager_create(
    max_depth: usize,
    coalesce_window_ms: u64,
) -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(Mutex::new(UndoHistoryState::new(
        max_depth,
        coalesce_window_ms,
    ))))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_destroy(state: *mut std::ffi::c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<Mutex<UndoHistoryState>>())) };
    }
}

unsafe fn undo_state<'a>(state: *const std::ffi::c_void) -> Option<&'a Mutex<UndoHistoryState>> {
    if state.is_null() {
        None
    } else {
        Some(unsafe { &*state.cast::<Mutex<UndoHistoryState>>() })
    }
}

fn read_name(name: *const u8, name_len: usize) -> Option<String> {
    if name_len == 0 {
        return Some(String::new());
    }
    if name.is_null() {
        return None;
    }
    Some(String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(name, name_len) }).into_owned())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_record(
    state: *mut std::ffi::c_void,
    name: *const u8,
    name_len: usize,
    undo_callback: u64,
    redo_callback: u64,
    timestamp_ms: u64,
    coalesce: bool,
) -> bool {
    let Some(state) = (unsafe { undo_state(state) }) else { return false; };
    let Some(name) = read_name(name, name_len) else { return false; };
    if undo_callback == 0 || redo_callback == 0 {
        return false;
    }
    let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state.record(ActionRecord { name, undo_callback, redo_callback, timestamp_ms }, coalesce);
    state.pending_callbacks.remove(&undo_callback);
    state.pending_callbacks.remove(&redo_callback);
    state.cleanup_native_callbacks();
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_register_native_callback(
    state: *mut std::ffi::c_void,
    context: *mut c_void,
    invoke: Option<NativeCallbackFn>,
    destroy: Option<NativeCallbackFn>,
) -> u64 {
    let Some(state) = (unsafe { undo_state(state) }) else { return 0; };
    let (Some(invoke), Some(destroy)) = (invoke, destroy) else { return 0; };
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
        .register_callback(context, invoke, destroy)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_invoke_native_callback(
    state: *mut std::ffi::c_void,
    callback_id: u64,
) -> bool {
    let Some(state) = (unsafe { undo_state(state) }) else { return false; };
    invoke_native_callbacks(state, &[callback_id])
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_discard_native_callback(
    state: *mut std::ffi::c_void,
    callback_id: u64,
) {
    let Some(state) = (unsafe { undo_state(state) }) else { return; };
    let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state.pending_callbacks.remove(&callback_id);
    state.cleanup_native_callbacks();
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_apply_and_invoke(
    state: *mut std::ffi::c_void,
    redo: bool,
) -> bool {
    let Some(state_mutex) = (unsafe { undo_state(state) }) else { return false; };
    let callback_ids = {
        let mut state = state_mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let required = state.next_callback_count(redo);
        if required == 0 { return false; }
        let mut callback_ids = vec![0; required];
        let stack = if redo { &state.redo } else { &state.undo };
        let Some(entry) = stack.back() else { return false; };
        let mut expected_ids = Vec::with_capacity(required);
        entry.callback_ids(redo, &mut expected_ids);
        let Some(callbacks) = callbacks_for(&state, &expected_ids) else { return false; };
        if state.apply_history(redo, &mut callback_ids) != required {
            return false;
        }
        state.cleanup_native_callbacks();
        callbacks
    };
    invoke_callbacks(callback_ids);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_abort_and_invoke(
    state: *mut std::ffi::c_void,
) -> bool {
    let Some(state_mutex) = (unsafe { undo_state(state) }) else { return false; };
    let callbacks = {
        let mut state = state_mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((_, actions)) = state.transaction.as_ref() else { return false; };
        let required = actions.len();
        let expected_ids = actions.iter().rev().map(|action| action.undo_callback).collect::<Vec<_>>();
        let Some(callbacks) = callbacks_for(&state, &expected_ids) else { return false; };
        let mut callback_ids = vec![0; required];
        state.abort_transaction(&mut callback_ids).unwrap_or(0);
        state.cleanup_native_callbacks();
        callbacks
    };
    invoke_callbacks(callbacks);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_begin_transaction(
    state: *mut std::ffi::c_void,
    name: *const u8,
    name_len: usize,
) -> bool {
    let Some(state) = (unsafe { undo_state(state) }) else { return false; };
    let Some(name) = read_name(name, name_len) else { return false; };
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).begin_transaction(name);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_end_transaction(
    state: *mut std::ffi::c_void,
) -> bool {
    let Some(state) = (unsafe { undo_state(state) }) else { return false; };
    let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let ended = state.end_transaction();
    state.cleanup_native_callbacks();
    ended
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_transaction_active(
    state: *const std::ffi::c_void,
) -> bool {
    let Some(state) = (unsafe { undo_state(state) }) else { return false; };
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).transaction.is_some()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_undo_count(
    state: *const std::ffi::c_void,
) -> usize {
    let Some(state) = (unsafe { undo_state(state) }) else { return 0; };
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).undo.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_redo_count(
    state: *const std::ffi::c_void,
) -> usize {
    let Some(state) = (unsafe { undo_state(state) }) else { return 0; };
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).redo.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_apply_history(
    state: *mut std::ffi::c_void,
    redo: bool,
    output: *mut u64,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { undo_state(state) }) else { return 0; };
    let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let required = state.next_callback_count(redo);
    if required == 0 || output.is_null() || capacity < required {
        return required;
    }
    state.apply_history(redo, unsafe { std::slice::from_raw_parts_mut(output, capacity) })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_abort_transaction(
    state: *mut std::ffi::c_void,
    output: *mut u64,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { undo_state(state) }) else { return 0; };
    let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(required) = state.transaction.as_ref().map(|(_, actions)| actions.len()) else { return 0; };
    if required > capacity || (required > 0 && output.is_null()) {
        return required;
    }
    let output = if required == 0 { &mut [] } else {
        unsafe { std::slice::from_raw_parts_mut(output, capacity) }
    };
    state.abort_transaction(output).unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_top_name(
    state: *const std::ffi::c_void,
    redo: bool,
    output: *mut u8,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { undo_state(state) }) else { return 0; };
    let state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let stack = if redo { &state.redo } else { &state.undo };
    let Some(entry) = stack.back() else { return 0; };
    let name = entry.name().as_bytes();
    if !output.is_null() {
        let count = capacity.min(name.len());
        unsafe { std::slice::from_raw_parts_mut(output, count).copy_from_slice(&name[..count]) };
    }
    name.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_live_callback_count(
    state: *const std::ffi::c_void,
) -> usize {
    let Some(state) = (unsafe { undo_state(state) }) else { return 0; };
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).live_callback_ids().len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_copy_live_callbacks(
    state: *const std::ffi::c_void,
    output: *mut u64,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { undo_state(state) }) else { return 0; };
    let callbacks = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).live_callback_ids();
    if output.is_null() || capacity < callbacks.len() {
        return callbacks.len();
    }
    unsafe { std::slice::from_raw_parts_mut(output, callbacks.len()).copy_from_slice(&callbacks) };
    callbacks.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_undo_manager_clear(state: *mut std::ffi::c_void) {
    let Some(state) = (unsafe { undo_state(state) }) else { return; };
    let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state.undo.clear();
    state.redo.clear();
    state.transaction = None;
    state.pending_callbacks.clear();
    state.native_callbacks.clear();
}

#[cfg(test)]
mod tests {
    use super::{
        hirari_undo_manager_abort_transaction, hirari_undo_manager_apply_history,
        hirari_undo_manager_begin_transaction, hirari_undo_manager_create,
        hirari_undo_manager_destroy, hirari_undo_manager_end_transaction,
        hirari_undo_manager_live_callback_count, hirari_undo_manager_record,
        hirari_undo_manager_undo_count,
    };

    unsafe fn record(state: *mut std::ffi::c_void, name: &str, undo: u64, redo: u64, time: u64, coalesce: bool) {
        assert!(unsafe {
            hirari_undo_manager_record(
                state, name.as_ptr(), name.len(), undo, redo, time, coalesce,
            )
        });
    }

    #[test]
    fn coalescing_preserves_first_undo_and_latest_redo_callback() {
        unsafe {
            let state = hirari_undo_manager_create(128, 300);
            record(state, "gain", 1, 2, 1000, true);
            record(state, "gain", 3, 4, 1100, true);
            assert_eq!(hirari_undo_manager_undo_count(state), 1);
            assert_eq!(hirari_undo_manager_live_callback_count(state), 2);
            let mut callbacks = [0u64; 2];
            assert_eq!(hirari_undo_manager_apply_history(state, false, callbacks.as_mut_ptr(), 2), 1);
            assert_eq!(callbacks[0], 1);
            hirari_undo_manager_destroy(state);
        }
    }

    #[test]
    fn transaction_undo_runs_reverse_and_redo_runs_forward() {
        unsafe {
            let state = hirari_undo_manager_create(128, 300);
            hirari_undo_manager_begin_transaction(state, b"compound".as_ptr(), 8);
            record(state, "one", 10, 11, 10, true);
            record(state, "two", 20, 21, 11, true);
            assert!(hirari_undo_manager_end_transaction(state));
            let mut callbacks = [0u64; 2];
            assert_eq!(hirari_undo_manager_apply_history(state, false, callbacks.as_mut_ptr(), 2), 2);
            assert_eq!(callbacks, [20, 10]);
            assert_eq!(hirari_undo_manager_apply_history(state, true, callbacks.as_mut_ptr(), 2), 2);
            assert_eq!(callbacks, [11, 21]);
            hirari_undo_manager_destroy(state);
        }
    }

    #[test]
    fn abort_rolls_back_and_depth_eviction_releases_callbacks() {
        unsafe {
            let state = hirari_undo_manager_create(1, 300);
            hirari_undo_manager_begin_transaction(state, b"abort".as_ptr(), 5);
            record(state, "one", 1, 2, 10, false);
            record(state, "two", 3, 4, 11, false);
            let mut callbacks = [0u64; 2];
            assert_eq!(hirari_undo_manager_abort_transaction(state, callbacks.as_mut_ptr(), 2), 2);
            assert_eq!(callbacks, [3, 1]);
            record(state, "one", 5, 6, 20, false);
            record(state, "two", 7, 8, 30, false);
            assert_eq!(hirari_undo_manager_undo_count(state), 1);
            assert_eq!(hirari_undo_manager_live_callback_count(state), 2);
            hirari_undo_manager_destroy(state);
        }
    }

    #[test]
    fn cxx_engine_transactions_invoke_rust_managed_callback_ids() {
        let engine = crate::ffi::new_audio_engine_offline();
        let engine = engine.as_ref().expect("offline engine should be created");
        let initial_tempo = engine.get_tempo();
        let initial_undo_count = engine.get_undo_count();

        engine.begin_undo_transaction("tempo transaction through Rust history");
        assert!(engine.set_tempo(96.0));
        assert!(engine.set_tempo(84.0));
        assert!(engine.end_undo_transaction());
        assert_eq!(engine.get_undo_count(), initial_undo_count + 1);
        assert_eq!(engine.get_tempo(), 84.0);

        engine.undo();
        assert_eq!(engine.get_tempo(), initial_tempo);
        engine.redo();
        assert_eq!(engine.get_tempo(), 84.0);
    }
}
