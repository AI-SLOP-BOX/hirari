//! CoreAudio HAL property access and CoreFoundation helpers.
use super::*;

pub(super) fn property_address(
    selector: u32,
    scope: u32,
    element: u32,
) -> sys::AudioObjectPropertyAddress {
    sys::AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: element,
    }
}

pub(super) unsafe fn read_property<T: Copy>(
    object: u32,
    selector: u32,
    scope: u32,
    element: u32,
) -> Result<T, i32> {
    let address = property_address(selector, scope, element);
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    let status = sys::AudioObjectGetPropertyData(
        object,
        &address,
        0,
        ptr::null(),
        &mut size,
        value.as_mut_ptr().cast::<c_void>(),
    );
    if status != 0 || size < size_of::<T>() as u32 {
        Err(status)
    } else {
        Ok(value.assume_init())
    }
}

pub(super) unsafe fn read_cfstring(object: u32, selector: u32, scope: u32, element: u32) -> Option<String> {
    let address = property_address(selector, scope, element);
    let mut value: sys::CFStringRef = ptr::null();
    let mut size = size_of::<sys::CFStringRef>() as u32;
    let status = sys::AudioObjectGetPropertyData(
        object,
        &address,
        0,
        ptr::null(),
        &mut size,
        (&mut value as *mut sys::CFStringRef).cast::<c_void>(),
    );
    if status != 0 || value.is_null() {
        return None;
    }
    cfstring_to_string(value)
}

pub(super) unsafe fn cfstring_to_string(value: sys::CFStringRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let mut buffer = [0 as c_char; 512];
    let converted = sys::CFStringGetCString(
        value,
        buffer.as_mut_ptr(),
        buffer.len() as sys::CFIndex,
        sys::kCFStringEncodingUTF8,
    );
    sys::CFRelease(value.cast::<c_void>());
    if converted == 0 {
        return None;
    }
    let text = CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Owned CoreFoundation reference. Every object built for the aggregate
/// description is released through this on scope exit, so no failure path can
/// leak one.
pub(super) struct CfRef(sys::CFTypeRef);

impl CfRef {
    fn new(value: sys::CFTypeRef) -> Option<Self> {
        if value.is_null() {
            None
        } else {
            Some(Self(value))
        }
    }

    fn as_ptr(&self) -> sys::CFTypeRef {
        self.0
    }
}

impl Drop for CfRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                sys::CFRelease(self.0);
            }
        }
    }
}

pub(super) unsafe fn cf_string(text: &str) -> Option<CfRef> {
    let c_text = CString::new(text).ok()?;
    CfRef::new(
        sys::CFStringCreateWithCString(ptr::null(), c_text.as_ptr(), sys::kCFStringEncodingUTF8)
            .cast::<c_void>(),
    )
}

pub(super) unsafe fn cf_key(key: &[u8]) -> Option<CfRef> {
    let c_key = CStr::from_bytes_until_nul(key).ok()?;
    CfRef::new(
        sys::CFStringCreateWithCString(ptr::null(), c_key.as_ptr(), sys::kCFStringEncodingUTF8)
            .cast::<c_void>(),
    )
}

pub(super) unsafe fn cf_dictionary(
    keys: &mut [sys::CFTypeRef],
    values: &mut [sys::CFTypeRef],
) -> Option<CfRef> {
    debug_assert_eq!(keys.len(), values.len());
    CfRef::new(
        sys::CFDictionaryCreate(
            ptr::null(),
            keys.as_mut_ptr(),
            values.as_mut_ptr(),
            keys.len() as sys::CFIndex,
            &sys::kCFTypeDictionaryKeyCallBacks,
            &sys::kCFTypeDictionaryValueCallBacks,
        )
        .cast::<c_void>(),
    )
}

pub(super) unsafe fn cf_array(values: &mut [sys::CFTypeRef]) -> Option<CfRef> {
    CfRef::new(
        sys::CFArrayCreate(
            ptr::null(),
            values.as_mut_ptr(),
            values.len() as sys::CFIndex,
            &sys::kCFTypeArrayCallBacks,
        )
        .cast::<c_void>(),
    )
}

pub(super) unsafe fn cf_i32(value: i32) -> Option<CfRef> {
    // The SDK documents these aggregate flags as CFNumbers; a CFBoolean here
    // is rejected by the aggregate builder.
    CfRef::new(
        sys::CFNumberCreate(
            ptr::null(),
            sys::kCFNumberSInt32Type as sys::CFNumberType,
            (&value as *const i32).cast::<c_void>(),
        )
        .cast::<c_void>(),
    )
}

/// Private aggregate UIDs are per-process and never persist, but a restart
/// inside one process must not collide with an aggregate whose asynchronous
/// teardown has not landed yet, so each creation gets its own suffix.
pub(super) fn private_aggregate_uid() -> String {
    static CREATION: AtomicU64 = AtomicU64::new(0);
    let sequence = CREATION.fetch_add(1, Ordering::Relaxed);
    format!("hirari-private-io-{}-{sequence}", std::process::id())
}

/// Compose a private aggregate device out of the two sub-devices named by the
/// plan and return its `AudioObjectID`. `AudioHardwareCreateAggregateDevice`
/// copies the description, so releasing every object here after the call is
/// correct; the caller owns the returned device and must destroy it with
/// `AudioHardwareDestroyAggregateDevice`.
pub(super) unsafe fn create_private_aggregate(composition: &DuplexComposition) -> Option<u32> {
    // Sub-device entries: [{ "uid": <input uid>, "drift": 1 },
    // { "uid": <output uid> }]. The output is the clock master, so the input
    // must compensate for clock drift or capture slowly slips against playback.
    let subdevice_key = cf_key(sys::kAudioSubDeviceUIDKey)?;
    let drift_key = cf_key(sys::kAudioSubDeviceDriftCompensationKey)?;
    let drift = cf_i32(1)?;
    let input_uid = cf_string(&composition.input_uid)?;
    let output_uid = cf_string(&composition.output_uid)?;

    let mut input_keys = [subdevice_key.as_ptr(), drift_key.as_ptr()];
    let mut input_values = [input_uid.as_ptr(), drift.as_ptr()];
    let input_entry = cf_dictionary(&mut input_keys, &mut input_values)?;
    let mut output_keys = [subdevice_key.as_ptr()];
    let mut output_values = [output_uid.as_ptr()];
    let output_entry = cf_dictionary(&mut output_keys, &mut output_values)?;
    let mut entries = [input_entry.as_ptr(), output_entry.as_ptr()];
    let subdevices = cf_array(&mut entries)?;

    // Description: uid/name/private/subdevices, with the output device acting
    // as both master and clock so playback and capture share one timebase.
    let uid = cf_string(&private_aggregate_uid())?;
    let name = cf_string("Hirari I/O")?;
    let private = cf_i32(1)?;
    let uid_key = cf_key(sys::kAudioAggregateDeviceUIDKey)?;
    let name_key = cf_key(sys::kAudioAggregateDeviceNameKey)?;
    let private_key = cf_key(sys::kAudioAggregateDeviceIsPrivateKey)?;
    let subdevices_key = cf_key(sys::kAudioAggregateDeviceSubDeviceListKey)?;
    let master_key = cf_key(sys::kAudioAggregateDeviceMainSubDeviceKey)?;
    let clock_key = cf_key(sys::kAudioAggregateDeviceClockDeviceKey)?;

    let mut keys = [
        uid_key.as_ptr(),
        name_key.as_ptr(),
        private_key.as_ptr(),
        subdevices_key.as_ptr(),
        master_key.as_ptr(),
        clock_key.as_ptr(),
    ];
    let mut values = [
        uid.as_ptr(),
        name.as_ptr(),
        private.as_ptr(),
        subdevices.as_ptr(),
        output_uid.as_ptr(),
        output_uid.as_ptr(),
    ];
    let description = cf_dictionary(&mut keys, &mut values)?;

    let mut aggregate: u32 = 0;
    let status = sys::AudioHardwareCreateAggregateDevice(
        description.as_ptr().cast::<sys::__CFDictionary>(),
        &mut aggregate,
    );
    if status != 0 || aggregate == 0 {
        None
    } else {
        Some(aggregate)
    }
}

/// Best-effort: a sub-device that refuses the requested rate is left alone and
/// the aggregate simply inherits whatever clock the master device settles on.
pub(super) unsafe fn set_nominal_sample_rate(device: u32, sample_rate: f64) -> Result<(), i32> {
    let address = property_address(
        sys::kAudioDevicePropertyNominalSampleRate,
        sys::kAudioObjectPropertyScopeGlobal,
        sys::kAudioObjectPropertyElementMain,
    );
    let status = sys::AudioObjectSetPropertyData(
        device,
        &address,
        0,
        ptr::null(),
        size_of::<f64>() as u32,
        (&sample_rate as *const f64).cast::<c_void>(),
    );
    if status != 0 {
        Err(status)
    } else {
        Ok(())
    }
}

/// Destroy a composed aggregate, if one is parked in the given slot.
pub(super) fn destroy_aggregate(aggregate: u32) {
    if aggregate != 0 {
        unsafe {
            let _ = sys::AudioHardwareDestroyAggregateDevice(aggregate);
        }
    }
}

pub(super) fn append_json_string(json: &mut String, value: &str) {
    json.push('"');
    for character in value.chars() {
        match character {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '\n' => json.push_str("\\n"),
            '\r' => json.push_str("\\r"),
            '\t' => json.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                json.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => json.push(other),
        }
    }
    json.push('"');
}

pub(super) unsafe fn stream_channel_count(device: u32, scope: u32) -> u32 {
    let address = property_address(sys::kAudioDevicePropertyStreamConfiguration, scope, sys::kAudioObjectPropertyElementMain);
    let mut size = 0u32;
    if sys::AudioObjectGetPropertyDataSize(
        device,
        &address,
        0,
        ptr::null(),
        &mut size,
    ) != 0
        || (size as usize) < offset_of!(sys::AudioBufferList, mBuffers)
        || size as usize > 64 * 1024
    {
        return 0;
    }
    let mut storage = vec![0u8; size as usize];
    if sys::AudioObjectGetPropertyData(
        device,
        &address,
        0,
        ptr::null(),
        &mut size,
        storage.as_mut_ptr().cast::<c_void>(),
    ) != 0
        || (size as usize) < offset_of!(sys::AudioBufferList, mBuffers)
    {
        return 0;
    }
    let list = &*(storage.as_ptr().cast::<sys::AudioBufferList>());
    let capacity = (size as usize - offset_of!(sys::AudioBufferList, mBuffers))
        / size_of::<sys::AudioBuffer>();
    if list.mNumberBuffers as usize > capacity {
        return 0;
    }
    let mut channels = 0u32;
    for index in 0..list.mNumberBuffers as usize {
        let buffer = &*list.mBuffers.as_ptr().add(index);
        let count = buffer.mNumberChannels;
        if count > 256u32.saturating_sub(channels) {
            return 0;
        }
        channels += count;
    }
    channels
}

pub(super) unsafe fn channel_names_for_scope(
    device: u32,
    scope: u32,
    channel_count: u32,
    fallback_prefix: &str,
) -> Vec<String> {
    let mut names = Vec::with_capacity(channel_count as usize);
    for channel in 1..=channel_count {
        let name = read_cfstring(
            device,
            sys::kAudioObjectPropertyElementName,
            scope,
            channel,
        );
        names.push(name.unwrap_or_else(|| format!("{fallback_prefix} {channel}")));
    }
    names
}

