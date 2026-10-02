//! XML attribute escaping for the persistence boundary.
//!
//! Project paths and region names routinely contain ampersands, quotes, or
//! angle brackets. Writing them verbatim into an attribute produces a file that
//! cannot be reopened, so every attribute value crosses this boundary escaped.
//!
//! [`unescape_xml`] reproduces the decode order the exporter's encoder implies.
//! It is deliberately a single ordered pass over the five known entities rather
//! than a recursive expansion: a value that legitimately contains the literal
//! text `&amp;lt;` decodes to `&lt;` and no further, matching the behavior the
//! persisted format has always had. Changing that would silently alter every
//! project written by earlier versions.

/// Escapes the five characters that are significant in an XML attribute value.
pub fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Entities in decode order. The order matters: `&amp;` is decoded last so that
/// a `&` produced by an earlier replacement is never re-expanded.
const ENTITIES: [(&str, &str); 5] = [
    ("&quot;", "\""),
    ("&apos;", "'"),
    ("&gt;", ">"),
    ("&lt;", "<"),
    ("&amp;", "&"),
];

/// Reverses [`escape_xml`] for a single pass over each known entity.
pub fn unescape_xml(value: &str) -> String {
    let mut value = value.to_owned();
    for (entity, replacement) in ENTITIES {
        let mut offset = 0usize;
        while let Some(found) = value[offset..].find(entity) {
            let at = offset + found;
            value.replace_range(at..at + entity.len(), replacement);
            offset = at + replacement.len();
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::{escape_xml, unescape_xml};

    /// The probe the native contract has always round-tripped.
    const PROBE: &str = "C:\\Audio & Mix/<take>\"lead\"'";

    #[test]
    fn escapes_every_significant_character() {
        assert_eq!(escape_xml("&"), "&amp;");
        assert_eq!(escape_xml("<"), "&lt;");
        assert_eq!(escape_xml(">"), "&gt;");
        assert_eq!(escape_xml("\""), "&quot;");
        assert_eq!(escape_xml("'"), "&apos;");
        assert_eq!(escape_xml("plain"), "plain");
        assert_eq!(
            escape_xml(PROBE),
            "C:\\Audio &amp; Mix/&lt;take&gt;&quot;lead&quot;&apos;"
        );
    }

    #[test]
    fn round_trips_the_contract_probe() {
        assert_eq!(unescape_xml(&escape_xml(PROBE)), PROBE);
    }

    /// Characterizes the single-pass decode order. Each expectation below was
    /// taken from the C++ implementation this module replaces.
    #[test]
    fn unescape_is_a_single_ordered_pass() {
        assert_eq!(unescape_xml("&amp;"), "&");
        // A `&` immediately before an entity survives: only the entity itself
        // is replaced. Measured against the retired C++ implementation.
        assert_eq!(unescape_xml("&&amp;"), "&&");
        assert_eq!(unescape_xml("&amp;lt;"), "&lt;");
        assert_eq!(unescape_xml("&amp;amp;"), "&amp;");
        assert_eq!(unescape_xml("&amp;quot;"), "&quot;");
        assert_eq!(unescape_xml("&notanentity;"), "&notanentity;");
        assert_eq!(unescape_xml("&AMP"), "&AMP");
        assert_eq!(unescape_xml("trailing&"), "trailing&");
        assert_eq!(unescape_xml("&amp"), "&amp");
        assert_eq!(unescape_xml("&&&&"), "&&&&");
        assert_eq!(unescape_xml("&&&amp;&&quot;"), "&&&&\"");
    }

    #[test]
    fn escape_then_unescape_is_identity_for_bare_characters() {
        for value in ["plain", "a&b", "x<y>z", "q\"r", "s't"] {
            assert_eq!(unescape_xml(&escape_xml(value)), value);
        }
    }
}

// --- C ABI for the native contract -------------------------------------

use std::os::raw::c_char;

/// Copies `input` into a NUL-terminated heap buffer the caller frees with
/// [`hirari_xml_string_free`]. Returns null only when `input` is null.
#[no_mangle]
pub unsafe extern "C" fn hirari_xml_escape_attribute(input: *const c_char) -> *mut c_char {
    copy_out(unsafe { borrow_str(input) }, crate::xml_attribute::escape_xml)
}

/// Reverse of [`hirari_xml_escape_attribute`].
#[no_mangle]
pub unsafe extern "C" fn hirari_xml_unescape_attribute(input: *const c_char) -> *mut c_char {
    copy_out(
        unsafe { borrow_str(input) },
        crate::xml_attribute::unescape_xml,
    )
}

/// Releases a buffer returned by one of the functions above.
#[no_mangle]
pub unsafe extern "C" fn hirari_xml_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { std::ffi::CString::from_raw(value) });
    }
}

unsafe fn borrow_str(input: *const c_char) -> Option<&'static str> {
    if input.is_null() {
        return None;
    }
    unsafe { std::ffi::CStr::from_ptr(input) }.to_str().ok()
}

fn copy_out<F>(input: Option<&str>, transform: F) -> *mut c_char
where
    F: FnOnce(&str) -> String,
{
    let Some(input) = input else {
        return std::ptr::null_mut();
    };
    match std::ffi::CString::new(transform(input)) {
        Ok(owned) => owned.into_raw(),
        // A NUL inside the escaped form cannot happen for these entities, but
        // never hand back a lossy conversion if it ever did.
        Err(_) => std::ptr::null_mut(),
    }
}
