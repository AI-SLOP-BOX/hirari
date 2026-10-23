use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const MAX_RECORDS: usize = 10_000;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct Record {
    timestamp: u64,
    category: Vec<u8>,
    key: Vec<u8>,
    value: Vec<u8>,
}

struct ProjectDb {
    state: Mutex<DbState>,
}

struct DbState {
    records: Vec<Record>,
    category_index: HashMap<Vec<u8>, Vec<usize>>,
}

#[no_mangle]
pub extern "C" fn hirari_project_db_create() -> *mut c_void {
    Box::into_raw(Box::new(ProjectDb { state: Mutex::new(DbState {
        records: Vec::new(), category_index: HashMap::new(),
    }) })).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_project_db_destroy(handle: *mut c_void) {
    if !handle.is_null() { drop(Box::from_raw(handle.cast::<ProjectDb>())); }
}

unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 { Some(&[]) } else if ptr.is_null() { None } else { Some(std::slice::from_raw_parts(ptr, len)) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_project_db_record(
    handle: *mut c_void, timestamp: u64,
    category: *const u8, category_len: usize,
    key: *const u8, key_len: usize,
    value: *const u8, value_len: usize,
) -> bool {
    let Some(db) = handle.cast::<ProjectDb>().as_ref() else { return false };
    let (Some(category), Some(key), Some(value)) = (bytes(category, category_len), bytes(key, key_len), bytes(value, value_len)) else { return false };
    let Ok(mut state) = db.state.lock() else { return false };
    if state.records.len() == MAX_RECORDS {
        state.records.remove(0);
        state.category_index.clear();
        let rebuilt = state.records.iter().enumerate()
            .map(|(index, record)| (record.category.clone(), index))
            .collect::<Vec<_>>();
        for (category, index) in rebuilt {
            state.category_index.entry(category).or_default().push(index);
        }
    }
    let index = state.records.len();
    let record = Record { timestamp, category: category.to_vec(), key: key.to_vec(), value: value.to_vec() };
    state.category_index.entry(record.category.clone()).or_default().push(index);
    state.records.push(record);
    true
}

pub type RecordCallback = unsafe extern "C" fn(*mut c_void, u64, *const u8, usize, *const u8, usize, *const u8, usize);

#[no_mangle]
pub unsafe extern "C" fn hirari_project_db_query(
    handle: *const c_void, category: *const u8, category_len: usize,
    context: *mut c_void, callback: Option<RecordCallback>,
) -> bool {
    let Some(db) = handle.cast::<ProjectDb>().as_ref() else { return false };
    let Some(category) = bytes(category, category_len) else { return false };
    let Some(callback) = callback else { return false };
    let Ok(state) = db.state.lock() else { return false };
    let Some(indices) = state.category_index.get(category) else { return true };
    for &index in indices {
        let Some(r) = state.records.get(index) else { continue };
        callback(context, r.timestamp, r.category.as_ptr(), r.category.len(), r.key.as_ptr(), r.key.len(), r.value.as_ptr(), r.value.len());
    }
    true
}

fn write_u32(file: &mut File, value: u32) -> std::io::Result<()> { file.write_all(&value.to_ne_bytes()) }

#[no_mangle]
pub unsafe extern "C" fn hirari_project_db_flush(handle: *const c_void, path: *const u8, path_len: usize) -> bool {
    let Some(db) = handle.cast::<ProjectDb>().as_ref() else { return false };
    let Some(path_bytes) = bytes(path, path_len) else { return false };
    if path_bytes.is_empty() { return false; }
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;
    #[cfg(unix)]
    let destination = Path::new(std::ffi::OsStr::from_bytes(path_bytes));
    #[cfg(not(unix))]
    let destination = match std::str::from_utf8(path_bytes) { Ok(p) => Path::new(p), Err(_) => return false };
    let Ok(snapshot) = db.state.lock().map(|state| state.records.clone()) else { return false };
    let Some(parent) = destination.parent() else { return false };
    let seq = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut temporary_name = destination.as_os_str().to_os_string();
    #[cfg(unix)]
    temporary_name.push(format!(".tmp-{}-{}", std::process::id(), seq));
    #[cfg(windows)]
    temporary_name.push(format!(".tmp-0-{}", seq));
    #[cfg(not(any(unix, windows)))]
    temporary_name.push(format!(".tmp-0-{}", seq));
    let temporary = Path::new(&temporary_name);
    let result = (|| -> std::io::Result<()> {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(temporary)?;
        f.write_all(b"AURA\x01DB")?;
        write_u32(&mut f, 1)?;
        write_u32(&mut f, snapshot.len().min(MAX_RECORDS) as u32)?;
        for r in snapshot.iter().take(MAX_RECORDS) {
            f.write_all(&r.timestamp.to_ne_bytes())?;
            let cat = &r.category[..r.category.len().min(4096)];
            let key = &r.key[..r.key.len().min(4096)];
            let value = &r.value[..r.value.len().min(1 << 20)];
            write_u32(&mut f, cat.len() as u32)?; f.write_all(cat)?;
            write_u32(&mut f, key.len() as u32)?; f.write_all(key)?;
            write_u32(&mut f, value.len() as u32)?; f.write_all(value)?;
        }
        f.flush()?; f.sync_all()?; drop(f);
        fs::rename(temporary, destination)?;
        #[cfg(unix)] File::open(if parent.as_os_str().is_empty() { Path::new(".") } else { parent })?.sync_all()?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(temporary); return false; }
    true
}
