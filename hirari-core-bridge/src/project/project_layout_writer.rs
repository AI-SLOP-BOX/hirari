//! Layout JSON writer core: the scratch buffer and the primitive
//! emission helpers, split out of project_layout.rs.
use super::*;

pub(crate) struct LayoutWriter<'a> {
    pub(crate) out: Vec<u8>,
    pub(crate) cancel: Option<(unsafe extern "C" fn(*const c_void) -> bool, *const c_void)>,
    pub(crate) sidechain_routes: &'a [HirariLayoutSidechainRoute],
    pub(crate) feedback_routes: &'a [HirariLayoutFeedbackRoute],
}

impl LayoutWriter<'_> {
    pub(crate) fn cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|(is_cancelled, context)| unsafe { is_cancelled(context) })
    }

    pub(crate) fn raw(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

    pub(crate) fn text(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match byte {
                b'"' => self.out.extend_from_slice(b"\\\""),
                b'\\' => self.out.extend_from_slice(b"\\\\"),
                b'\n' => self.out.extend_from_slice(b"\\n"),
                b'\r' => self.out.extend_from_slice(b"\\r"),
                b'\t' => self.out.extend_from_slice(b"\\t"),
                control if control < 0x20 => self.out.push(b'?'),
                other => self.out.push(other),
            }
        }
    }

    pub(crate) fn number(&mut self, value: f64) {
        let text = if value.is_finite() {
            format!("{value:.6}")
        } else {
            String::from("0")
        };
        self.raw(text.as_bytes());
    }

    pub(crate) fn number32(&mut self, value: f32) {
        self.number(f64::from(value));
    }

    pub(crate) fn parameter(&mut self, value: f32) {
        let text = if value.is_finite() {
            format!("{value:.6}")
        } else {
            String::from("0.0")
        };
        self.raw(text.as_bytes());
    }

    pub(crate) fn boolean(&mut self, value: bool) {
        self.raw(if value { b"true" } else { b"false" });
    }

    pub(crate) fn hex(&mut self, bytes: &[u8]) {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for &byte in bytes {
            self.out.push(DIGITS[(byte >> 4) as usize]);
            self.out.push(DIGITS[(byte & 0x0f) as usize]);
        }
    }
}

pub(crate) unsafe fn optional_slice<'a, T>(data: *const T, count: usize) -> Option<&'a [T]> {
    if count == 0 {
        Some(&[])
    } else if data.is_null() {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(data, count) })
    }
}

pub(crate) unsafe fn string_bytes<'a>(value: &HirariLayoutString) -> Option<&'a [u8]> {
    unsafe { optional_slice(value.data, value.len) }
}
