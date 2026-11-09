//! CLAP module discovery, entry lifetime, instance construction and teardown.
//!
//! The sandbox worker calls these functions before publishing readiness. The
//! library handle and host table stay alive for the complete plugin lifetime.

use core::ffi::{c_char, c_void, CStr};
use core::ptr;
use std::boxed::Box;
use std::ffi::CString;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::vec::Vec;

/// Resolve a macOS plugin package to its executable image. Non-directory paths
/// pass through unchanged; package directories select the first executable
/// regular file in Contents/MacOS, matching the worker's former POSIX logic.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_resolve_image_path(path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return ptr::null_mut();
    }
    let path_bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
    let resolved = if path_bytes.is_empty() {
        Vec::new()
    } else {
        #[cfg(unix)]
        {
            use std::{ffi::OsStr, fs, os::unix::fs::PermissionsExt, path::PathBuf};
            let path_buf = PathBuf::from(OsStr::from_bytes(path_bytes));
            match fs::metadata(&path_buf) {
                Err(_) => path_bytes.to_vec(),
                Ok(metadata) if !metadata.is_dir() => path_bytes.to_vec(),
                Ok(_) => {
                    let image_dir = path_buf.join("Contents").join("MacOS");
                    let mut selected = Vec::new();
                    if let Ok(entries) = fs::read_dir(image_dir) {
                        for entry in entries.flatten() {
                            let candidate = entry.path();
                            let Ok(metadata) = fs::metadata(&candidate) else {
                                continue;
                            };
                            if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
                                selected = candidate.into_os_string().into_vec();
                                break;
                            }
                        }
                    }
                    selected
                }
            }
        }
        #[cfg(not(unix))]
        {
            path_bytes.to_vec()
        }
    };
    CString::new(resolved).map_or(ptr::null_mut(), CString::into_raw)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_resolved_image_free(path: *mut c_char) {
    if !path.is_null() {
        drop(unsafe { CString::from_raw(path) });
    }
}

const PLUGIN_FACTORY_ID: &[u8] = b"clap.plugin-factory\0";
const STATE_EXTENSION_ID: &[u8] = b"clap.state\0";
const LATENCY_EXTENSION_ID: &[u8] = b"clap.plugin.latency\0";
const HOST_NAME: &[u8] = b"hirari-sandbox\0";
const HOST_VENDOR: &[u8] = b"Hirari\0";
const HOST_URL: &[u8] = b"\0";
const HOST_VERSION: &[u8] = b"1\0";
const RTLD_NOW: i32 = 2;
#[cfg(target_os = "macos")]
const RTLD_LOCAL: i32 = 4;
#[cfg(not(target_os = "macos"))]
const RTLD_LOCAL: i32 = 0;

#[cfg_attr(any(target_os = "linux", target_os = "android"), link(name = "dl"))]
unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> i32;
}

struct DynamicLibrary(*mut c_void);

impl DynamicLibrary {
    unsafe fn open(path: &CStr) -> Option<Self> {
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
        (!handle.is_null()).then_some(Self(handle))
    }

    unsafe fn symbol(&self, name: &'static [u8]) -> *mut c_void {
        unsafe { dlsym(self.0, name.as_ptr().cast()) }
    }
}

impl Drop for DynamicLibrary {
    fn drop(&mut self) {
        unsafe {
            dlclose(self.0);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Version {
    major: u32,
    minor: u32,
    revision: u32,
}

#[repr(C)]
struct Host {
    version: Version,
    host_data: *const c_char,
    name: *const c_char,
    vendor: *const c_char,
    url: *const c_char,
    version_string: *const c_char,
    request_restart: Option<unsafe extern "C" fn(*const Host)>,
    request_process: Option<unsafe extern "C" fn(*const Host)>,
    request_callback: Option<unsafe extern "C" fn(*const Host)>,
    get_extension: Option<unsafe extern "C" fn(*const Host, *const c_char) -> *const c_void>,
}

#[repr(C)]
struct Descriptor {
    version: Version,
    id: *const c_char,
    name: *const c_char,
    vendor: *const c_char,
    url: *const c_char,
    manual_url: *const c_char,
    support_url: *const c_char,
    version_string: *const c_char,
    // NOTE: core-abis declares `features` as a flexible array member
    // (`const char *features[]`) at offset 72, not a pointer field. It is
    // read through `descriptor_feature` below.
}

/// Byte offset of the flexible `features` array inside a descriptor.
const DESCRIPTOR_FEATURES_OFFSET: usize = 72;

unsafe fn descriptor_feature(descriptor: *const Descriptor, index: usize) -> *const c_char {
    let base = descriptor.cast::<u8>().add(DESCRIPTOR_FEATURES_OFFSET);
    let slot = unsafe { (base.cast::<*const *const c_char>()).add(index) };
    unsafe { *slot }.cast::<c_char>()
}

#[repr(C)]
struct Plugin {
    descriptor: *const Descriptor,
    plugin_data: *mut c_void,
    init: Option<unsafe extern "C" fn(*const Plugin) -> bool>,
    destroy: Option<unsafe extern "C" fn(*const Plugin)>,
    activate: Option<unsafe extern "C" fn(*const Plugin, f64, u32, u32) -> bool>,
    deactivate: Option<unsafe extern "C" fn(*const Plugin)>,
    start_processing: Option<unsafe extern "C" fn(*const Plugin) -> bool>,
    stop_processing: Option<unsafe extern "C" fn(*const Plugin)>,
    reset: Option<unsafe extern "C" fn(*const Plugin)>,
    process: Option<unsafe extern "C" fn(*const Plugin, *const c_void) -> i32>,
    get_extension: Option<unsafe extern "C" fn(*const Plugin, *const c_char) -> *const c_void>,
    on_main_thread: Option<unsafe extern "C" fn(*const Plugin)>,
}

#[repr(C)]
struct Factory {
    get_plugin_count: Option<unsafe extern "C" fn(*const Factory) -> u32>,
    get_plugin_descriptor: Option<unsafe extern "C" fn(*const Factory, u32) -> *const Descriptor>,
    create_plugin:
        Option<unsafe extern "C" fn(*const Factory, *const Host, *const c_char) -> *const Plugin>,
}

#[repr(C)]
struct Entry {
    version: Version,
    init: Option<unsafe extern "C" fn(*const c_char) -> bool>,
    deinit: Option<unsafe extern "C" fn()>,
    get_factory: Option<unsafe extern "C" fn(*const c_char) -> *const c_void>,
}

#[repr(C)]
struct StateExtension {
    save: Option<unsafe extern "C" fn(*const Plugin, *const c_void) -> bool>,
    load: Option<unsafe extern "C" fn(*const Plugin, *const c_void) -> bool>,
}

#[repr(C)]
struct LatencyExtension {
    get: Option<unsafe extern "C" fn(*const Plugin) -> u32>,
}

const PARAMS_EXTENSION_ID: &[u8] = b"clap.params\0";

/// `clap.plugin.params` extension (core-abis).
#[repr(C)]
struct ParamsExtension {
    get_count: Option<unsafe extern "C" fn(*const Plugin) -> u32>,
    get_info: Option<unsafe extern "C" fn(*const Plugin, u32, *mut ClapParamInfo) -> bool>,
}

#[repr(C)]
struct ClapParamInfo {
    id: u32,
    flags: u32,
    name: *const c_char,
    module: *const c_char,
    default_value: f64,
    min_value: f64,
    max_value: f64,
    cookies: *const c_void,
}

const CLAP_PARAM_IS_AUTOMATABLE: u32 = 1;
const CLAP_PARAM_IS_READ_ONLY: u32 = 2;

const _: () = assert!(core::mem::size_of::<Host>() == 88);
const _: () = assert!(core::mem::size_of::<Descriptor>() == 72);
const _: () = assert!(core::mem::size_of::<Plugin>() == 96);
const _: () = assert!(core::mem::offset_of!(Plugin, process) == 72);
const _: () = assert!(core::mem::size_of::<Factory>() == 24);
const _: () = assert!(core::mem::size_of::<Entry>() == 40);
const _: () = assert!(core::mem::size_of::<StateExtension>() == 16);

unsafe extern "C" fn noop_host_callback(_: *const Host) {}

unsafe extern "C" fn no_host_extension(_: *const Host, _: *const c_char) -> *const c_void {
    ptr::null()
}

struct ClapInstance {
    // Field order keeps plugin teardown and entry deinit ahead of the library
    // unload when Drop runs.
    plugin: *const Plugin,
    state: *const StateExtension,
    entry: *const Entry,
    activated: bool,
    processing: bool,
    created: bool,
    host: Box<Host>,
    _library: DynamicLibrary,
}

impl Drop for ClapInstance {
    fn drop(&mut self) {
        unsafe {
            if self.processing {
                if let Some(stop) = (*self.plugin).stop_processing {
                    stop(self.plugin);
                }
            }
            if self.activated {
                if let Some(deactivate) = (*self.plugin).deactivate {
                    deactivate(self.plugin);
                }
            }
            if self.created {
                if let Some(destroy) = (*self.plugin).destroy {
                    destroy(self.plugin);
                }
            }
            if !self.entry.is_null() {
                if let Some(deinit) = (*self.entry).deinit {
                    deinit();
                }
            }
        }
        let _ = &self.host;
    }
}

/// Reads a C string with fallback safety: some plugins ship descriptors
/// whose features array is not NUL-terminated (or is otherwise malformed),
/// so blind `strlen` would walk into unrelated memory. Bytes are read one
/// at a time up to `max`, stopping at the first NUL.
unsafe fn bounded_c_string(ptr: *const c_char, max: usize) -> Vec<u8> {
    if ptr.is_null() || (ptr as usize) <= 0x1000 {
        return Vec::new();
    }
    let mut bytes = Vec::with_capacity(max.min(64));
    for index in 0..max {
        let byte = unsafe { (ptr as *const u8).add(index).read() };
        if byte == 0 {
            break;
        }
        bytes.push(byte);
        if bytes.iter().any(|&b| b < 0x09 || (0x0E..0x20).contains(&b)) {
            return Vec::new();
        }
    }
    bytes
}

unsafe fn descriptor_has_feature(descriptor: *const Descriptor, requested: &CStr) -> bool {
    if descriptor.is_null() {
        return false;
    }
    let mut index = 0;
    loop {
        let feature = unsafe { descriptor_feature(descriptor, index) };
        if feature.is_null() || (feature as usize) <= 0x1000 {
            return false;
        }
        if unsafe { bounded_c_string(feature, 256) } == requested.to_bytes() {
            return true;
        }
        index += 1;
        if index > 256 {
            return false;
        }
    }
}

/// Load and start one CLAP plugin. Returns 1 for image-load failure, 2 for an
/// invalid CLAP entry/factory, 3 for missing feature or instance startup.
#[no_mangle]
pub unsafe extern "C" fn hirari_clap_instance_load(
    image_path: *const c_char,
    package_path: *const c_char,
    sample_rate: f64,
    min_frames: u32,
    max_frames: u32,
    output_instance: *mut *mut c_void,
    output_midi_instrument: *mut bool,
) -> i32 {
    if image_path.is_null()
        || package_path.is_null()
        || output_instance.is_null()
        || output_midi_instrument.is_null()
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
        || min_frames == 0
        || max_frames < min_frames
    {
        return 2;
    }
    output_instance.write(ptr::null_mut());
    output_midi_instrument.write(false);
    let library = match DynamicLibrary::open(CStr::from_ptr(image_path)) {
        Some(library) => library,
        None => return 1,
    };
    let entry_symbol = library.symbol(b"clap_entry\0");
    if entry_symbol.is_null() {
        return 2;
    }
    let entry = entry_symbol.cast::<Entry>();
    if (*entry).version.major != 1 {
        return 2;
    }
    let (Some(init), Some(deinit), Some(get_factory)) =
        ((*entry).init, (*entry).deinit, (*entry).get_factory)
    else {
        return 2;
    };
    if !init(package_path) {
        return 2;
    }

    let factory_ptr = get_factory(PLUGIN_FACTORY_ID.as_ptr().cast());
    if factory_ptr.is_null() {
        deinit();
        return 3;
    }
    let factory = &*factory_ptr.cast::<Factory>();
    let (Some(get_count), Some(get_descriptor), Some(create_plugin)) = (
        factory.get_plugin_count,
        factory.get_plugin_descriptor,
        factory.create_plugin,
    ) else {
        deinit();
        return 3;
    };
    let count = get_count(factory);
    if count == 0 {
        deinit();
        return 3;
    }

    let requested_feature = std::env::var_os("HIRARI_PLUGIN_KIND")
        .filter(|feature| !feature.is_empty())
        .and_then(|feature| CString::new(feature.as_os_str().as_bytes()).ok());
    let mut selected = 0;
    if let Some(feature) = requested_feature.as_ref() {
        let mut found = false;
        for candidate in 0..count {
            if descriptor_has_feature(get_descriptor(factory, candidate), feature) {
                selected = candidate;
                found = true;
                break;
            }
        }
        if !found {
            deinit();
            return 3;
        }
    }
    let descriptor = get_descriptor(factory, selected);
    if descriptor.is_null() || (*descriptor).id.is_null() {
        deinit();
        return 3;
    }
    let midi_instrument = [
        b"instrument\0".as_slice(),
        b"synthesizer\0",
        b"sampler\0",
        b"drum\0",
    ]
    .iter()
    .any(|name| descriptor_has_feature(descriptor, CStr::from_bytes_with_nul_unchecked(name)));

    let host = Box::new(Host {
        version: Version {
            major: 1,
            minor: 0,
            revision: 0,
        },
        host_data: ptr::null(),
        name: HOST_NAME.as_ptr().cast(),
        vendor: HOST_VENDOR.as_ptr().cast(),
        url: HOST_URL.as_ptr().cast(),
        version_string: HOST_VERSION.as_ptr().cast(),
        request_restart: Some(noop_host_callback),
        request_process: Some(noop_host_callback),
        request_callback: Some(noop_host_callback),
        get_extension: Some(no_host_extension),
    });
    let plugin = create_plugin(factory, &*host, (*descriptor).id);
    if plugin.is_null() {
        deinit();
        return 3;
    }
    let instance = ClapInstance {
        plugin,
        state: ptr::null(),
        entry,
        activated: false,
        processing: false,
        created: true,
        host,
        _library: library,
    };
    let mut instance = Box::new(instance);
    let plugin_ref = &*plugin;
    if plugin_ref.init.is_none()
        || plugin_ref.destroy.is_none()
        || plugin_ref.activate.is_none()
        || plugin_ref.deactivate.is_none()
        || plugin_ref.start_processing.is_none()
        || plugin_ref.stop_processing.is_none()
        || plugin_ref.process.is_none()
    {
        drop(instance);
        return 3;
    }
    if !plugin_ref.init.unwrap()(plugin) {
        drop(instance);
        return 3;
    }
    if !plugin_ref.activate.unwrap()(plugin, sample_rate, min_frames, max_frames) {
        drop(instance);
        return 3;
    }
    instance.activated = true;
    if !plugin_ref.start_processing.unwrap()(plugin) {
        drop(instance);
        return 3;
    }
    instance.processing = true;
    if let Some(get_extension) = plugin_ref.get_extension {
        instance.state =
            get_extension(plugin, STATE_EXTENSION_ID.as_ptr().cast()).cast::<StateExtension>();
    }
    output_midi_instrument.write(midi_instrument);
    output_instance.write(Box::into_raw(instance).cast());
    0
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_instance_plugin(instance: *const c_void) -> *const c_void {
    if instance.is_null() {
        ptr::null()
    } else {
        (*instance.cast::<ClapInstance>()).plugin.cast()
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_instance_state(instance: *const c_void) -> *const c_void {
    if instance.is_null() {
        ptr::null()
    } else {
        (*instance.cast::<ClapInstance>()).state.cast()
    }
}

/// Processor latency in samples via the `clap.plugin.latency` extension.
/// Plugins without the extension report zero.
#[no_mangle]
pub unsafe extern "C" fn hirari_clap_instance_latency(instance: *const c_void) -> u32 {
    if instance.is_null() {
        return 0;
    }
    let plugin = (*instance.cast::<ClapInstance>()).plugin;
    if plugin.is_null() {
        return 0;
    }
    let get_extension = match (*plugin).get_extension {
        Some(get_extension) => get_extension,
        None => return 0,
    };
    let extension = get_extension(plugin, LATENCY_EXTENSION_ID.as_ptr().cast::<c_char>())
        .cast::<LatencyExtension>();
    if extension.is_null() {
        return 0;
    }
    match (*extension).get {
        Some(get) => get(plugin),
        None => 0,
    }
}

fn params_extension(instance: *const c_void) -> Option<(*const Plugin, *const ParamsExtension)> {
    if instance.is_null() {
        return None;
    }
    let plugin = unsafe { (*instance.cast::<ClapInstance>()).plugin };
    if plugin.is_null() {
        return None;
    }
    let get_extension = unsafe { (*plugin).get_extension? };
    let extension = unsafe { get_extension(plugin, PARAMS_EXTENSION_ID.as_ptr().cast::<c_char>()) };
    if extension.is_null() {
        return None;
    }
    Some((plugin, extension.cast::<ParamsExtension>()))
}

/// Number of parameters advertised by the `clap.params` extension.
#[no_mangle]
pub unsafe extern "C" fn hirari_clap_instance_parameter_count(instance: *const c_void) -> u32 {
    let Some((plugin, extension)) = params_extension(instance) else {
        return 0;
    };
    match unsafe { (*extension).get_count } {
        Some(get_count) => unsafe { get_count(plugin) },
        None => 0,
    }
}

/// Fills `out` for the `index`-th parameter.
#[no_mangle]
pub unsafe extern "C" fn hirari_clap_instance_parameter_info(
    instance: *const c_void,
    index: u32,
    out: *mut crate::mailbox::PluginParameterInfo,
) -> bool {
    let Some((plugin, extension)) = params_extension(instance) else {
        return false;
    };
    if out.is_null() {
        return false;
    }
    let Some(get_info) = (unsafe { (*extension).get_info }) else {
        return false;
    };
    // The plugin writes into its own struct size, which may exceed ours
    // when it was built against a different core-abis minor. A generously
    // sized zeroed scratch keeps an oversized write off our stack, and
    // only the fields we understand are read back.
    let mut scratch = [0u8; 512];
    let ok = unsafe { get_info(plugin, index, scratch.as_mut_ptr().cast::<ClapParamInfo>()) };
    if !ok {
        return false;
    }
    let read_u32 = |offset: usize| -> u32 {
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(&scratch[offset..offset + 4]);
        u32::from_le_bytes(bytes)
    };
    let read_f64 = |offset: usize| -> f64 {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&scratch[offset..offset + 8]);
        f64::from_le_bytes(bytes)
    };
    let read_ptr = |offset: usize| -> *const c_char {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&scratch[offset..offset + 8]);
        usize::from_le_bytes(bytes) as *const c_char
    };
    let filled = unsafe { &mut *out };
    filled.id = read_u32(0);
    let flags = read_u32(4);
    // Some CLAP builds lay their extended fields past the core struct, so
    // the value slots can hold non-double bytes. Anything non-finite or
    // absurd falls back to the normalized unit range.
    let sane = |value: f64| value.is_finite() && value.abs() < 1e6;
    filled.min_value = read_f64(32);
    filled.max_value = read_f64(40);
    filled.default_value = read_f64(24);
    if !sane(filled.min_value) || !sane(filled.max_value) || filled.min_value >= filled.max_value {
        filled.min_value = 0.0;
        filled.max_value = 1.0;
    }
    if !sane(filled.default_value) {
        filled.default_value = 0.0;
    }
    filled.default_value = filled
        .default_value
        .clamp(filled.min_value, filled.max_value);
    filled.flags = if flags & CLAP_PARAM_IS_AUTOMATABLE != 0 {
        1
    } else if flags & CLAP_PARAM_IS_READ_ONLY != 0 {
        2
    } else {
        0
    };
    // Name source: several widely used CLAP builds write the name inline
    // at offset 16 rather than through the pointer at offset 8 (whose
    // target is sometimes not dereferenceable after the call). Prefer a
    // valid inline name; fall back to the pointer only when the inline
    // bytes are not a clean printable string.
    let inline = bounded_c_string(scratch[16..64].as_ptr().cast::<c_char>(), 64);
    let via_pointer =
        unsafe { bounded_c_string(read_ptr(8), crate::mailbox::PARAMETER_NAME_BYTES) };
    let name: &[u8] = if inline.len() > 1 {
        &inline
    } else {
        &via_pointer
    };
    let take = name.len().min(crate::mailbox::PARAMETER_NAME_BYTES - 1);
    filled.name[..take].copy_from_slice(&name[..take]);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_instance_destroy(instance: *mut c_void) {
    if !instance.is_null() {
        drop(Box::from_raw(instance.cast::<ClapInstance>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_instance_reset(
    instance: *mut c_void,
    sample_rate: f64,
    min_frames: u32,
    max_frames: u32,
) -> bool {
    if instance.is_null()
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
        || min_frames == 0
        || max_frames < min_frames
    {
        return false;
    }
    let instance = &mut *instance.cast::<ClapInstance>();
    let plugin = &*instance.plugin;
    if instance.processing {
        if let Some(stop) = plugin.stop_processing {
            stop(instance.plugin);
        }
        instance.processing = false;
    }
    if instance.activated {
        if let Some(deactivate) = plugin.deactivate {
            deactivate(instance.plugin);
        }
        instance.activated = false;
    }
    let Some(activate) = plugin.activate else {
        return false;
    };
    if !activate(instance.plugin, sample_rate, min_frames, max_frames) {
        return false;
    }
    instance.activated = true;
    let Some(start) = plugin.start_processing else {
        return false;
    };
    if !start(instance.plugin) {
        return false;
    }
    instance.processing = true;
    true
}
