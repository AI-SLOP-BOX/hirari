//! Tests split out of midi_note_edit.rs.

    use super::*;

    fn valid() -> Vec<u64> {
        vec![3, 60, 100, 2, 960, 480, 100, 7, 4]
    }

    fn unpack(packed: &[u64]) -> Result<Vec<ScheduledMidiNoteFfi>, usize> {
        let mut notes = vec![ScheduledMidiNoteFfi::default(); packed.len() / 9 + 1];
        let count = unsafe {
            hirari_midi_notes_unpack(
                packed.as_ptr(),
                packed.len(),
                notes.as_mut_ptr(),
                notes.len(),
            )
        };
        if count == usize::MAX {
            return Err(count);
        }
        notes.truncate(count);
        Ok(notes)
    }

    #[test]
    fn decodes_every_field() {
        let notes = unpack(&valid()).expect("valid vector");
        assert_eq!(
            notes,
            vec![ScheduledMidiNoteFfi {
                track_id: 3,
                pitch: 60,
                velocity: 100,
                midi_channel: 2,
                articulation_id: 4,
                start_sample: 960,
                length_samples: 480,
                probability: 100,
                region_id: 7,
            }]
        );
    }

    #[test]
    fn rejects_a_vector_that_is_not_a_whole_number_of_notes() {
        assert!(unpack(&[1, 2, 3]).is_err());
    }

    #[test]
    fn rejects_out_of_range_fields() {
        for (index, value) in [
            (0, u64::from(u32::MAX) + 1),
            (1, 128),
            (2, 0),
            (2, 128),
            (3, 16),
            (5, 0),
            (6, 101),
            (7, u64::from(u32::MAX) + 1),
            (8, 128),
            (4, u64::MAX),
        ] {
            let mut packed = valid();
            packed[index] = value;
            assert!(unpack(&packed).is_err(), "field {index}={value} must fail");
        }
    }

    #[test]
    fn accepts_the_edge_of_every_range() {
        let mut packed = valid();
        packed[0] = u64::from(u32::MAX);
        packed[1] = 127;
        packed[2] = 1;
        packed[3] = 15;
        packed[4] = u64::MAX - 1;
        packed[5] = 1;
        packed[6] = 0;
        packed[7] = u64::from(u32::MAX);
        packed[8] = 127;
        assert_eq!(unpack(&packed).expect("edge values are valid").len(), 1);
    }

    #[test]
    fn count_request_does_not_write_when_capacity_is_short() {
        let packed = valid();
        let mut sentinel = [ScheduledMidiNoteFfi::default()];
        sentinel[0].pitch = 9;
        let count = unsafe {
            hirari_midi_notes_unpack(packed.as_ptr(), packed.len(), sentinel.as_mut_ptr(), 0)
        };
        assert_eq!(count, 1);
        assert_eq!(sentinel[0].pitch, 9);
    }

    #[test]
    fn packing_round_trips_the_decoded_note() {
        let notes = unpack(&valid()).expect("valid vector");
        let required = unsafe {
            hirari_midi_notes_pack(notes.as_ptr(), notes.len(), std::ptr::null_mut(), 0)
        };
        assert_eq!(required, valid().len());
        let mut packed = vec![0u64; required];
        assert_eq!(
            unsafe {
                hirari_midi_notes_pack(notes.as_ptr(), notes.len(), packed.as_mut_ptr(), packed.len())
            },
            required
        );
        assert_eq!(packed, valid());
    }

    #[test]
    fn packing_needs_room_for_every_field() {
        let notes = unpack(&valid()).expect("valid vector");
        let mut short = [7u64; 8];
        assert_eq!(
            unsafe {
                hirari_midi_notes_pack(notes.as_ptr(), notes.len(), short.as_mut_ptr(), short.len())
            },
            valid().len()
        );
        assert_eq!(short, [7u64; 8]);
    }

    #[test]
    fn empty_vector_reports_zero() {
        let count =
            unsafe { hirari_midi_notes_unpack(std::ptr::null(), 0, std::ptr::null_mut(), 0) };
        assert_eq!(count, 0);
    }
