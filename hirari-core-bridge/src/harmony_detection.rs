use std::collections::BTreeSet;

#[no_mangle]
pub unsafe extern "C" fn hirari_harmony_detect(
    notes: *const i32, count: usize, root_note: *mut i32, chord_type: *mut u8,
) -> bool {
    if notes.is_null() || root_note.is_null() || chord_type.is_null() || count == 0 { return false; }
    let notes = std::slice::from_raw_parts(notes, count);
    let pitch_classes = notes.iter().map(|note| note % 12).collect::<BTreeSet<_>>();
    let Some(&first) = pitch_classes.iter().next() else { return false };
    let (mut root, mut detected, mut best_score) = (first, 0u8, -1i32);
    for &candidate in &pitch_classes {
        let mut mask = 0u32;
        for &pitch in &pitch_classes {
            let interval = (pitch - candidate + 12) % 12;
            mask |= 1u32 << interval;
        }
        let (kind, score) = if mask & 0b10010001 == 0b10010001 && mask & (1 << 10) != 0 {
            (5, 7)
        } else if mask & 0b10010001 == 0b10010001 && mask & (1 << 11) != 0 {
            (6, 6)
        } else if mask & 0b10001001 == 0b10001001 && mask & (1 << 10) != 0 {
            (7, 5)
        } else if mask & 0b10010001 == 0b10010001 {
            (1, 4)
        } else if mask & 0b10001001 == 0b10001001 {
            (2, 4)
        } else if mask & 0b1001001 == 0b1001001 {
            (3, 3)
        } else if mask & 0b100010001 == 0b100010001 {
            (4, 3)
        } else { (0, 0) };
        if score > best_score { best_score = score; root = candidate; detected = kind; }
    }
    *root_note = root;
    *chord_type = detected;
    true
}
