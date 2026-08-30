//! Minimal, strict MIDI 2.0 UMP decoder for Channel Voice 64-bit packets.
//! Unsupported UMP message types are rejected rather than being misread as
//! MIDI 1.0 bytes.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UmpEvent {
    NoteOff {
        group: u8,
        channel: u8,
        note: u8,
        velocity: u16,
    },
    NoteOn {
        group: u8,
        channel: u8,
        note: u8,
        velocity: u16,
    },
    ControlChange {
        group: u8,
        channel: u8,
        controller: u8,
        value: u32,
    },
    /// MIDI 2.0 Registered Controller (RPN). `parameter` preserves the full
    /// 14-bit bank/index address used by Cubase controller lanes.
    RegisteredController {
        group: u8,
        channel: u8,
        parameter: u16,
        value: u32,
    },
    /// MIDI 2.0 Assignable Controller (NRPN).
    AssignableController {
        group: u8,
        channel: u8,
        parameter: u16,
        value: u32,
    },
    /// Relative MIDI 2.0 registered controller. The wire payload is a signed
    /// two's-complement increment/decrement, not an absolute position.
    RelativeRegisteredController {
        group: u8,
        channel: u8,
        parameter: u16,
        delta: i32,
    },
    /// Relative MIDI 2.0 assignable controller.
    RelativeAssignableController {
        group: u8,
        channel: u8,
        parameter: u16,
        delta: i32,
    },
    PerNotePitchBend {
        group: u8,
        channel: u8,
        note: u8,
        value: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UmpError {
    WrongPacketLength,
    UnsupportedMessageType,
    InvalidField,
}

pub fn encode_channel_voice(event: &UmpEvent) -> Result<[u8; 8], UmpError> {
    let (group, channel, status, data, value) = match *event {
        UmpEvent::NoteOff {
            group,
            channel,
            note,
            velocity,
        } => (
            group,
            channel,
            0x8,
            u16::from(note) << 8,
            (velocity as u32) << 16,
        ),
        UmpEvent::NoteOn {
            group,
            channel,
            note,
            velocity,
        } => (
            group,
            channel,
            0x9,
            u16::from(note) << 8,
            (velocity as u32) << 16,
        ),
        UmpEvent::ControlChange {
            group,
            channel,
            controller,
            value,
        } => (group, channel, 0xB, u16::from(controller) << 8, value),
        UmpEvent::RegisteredController {
            group,
            channel,
            parameter,
            value,
        } => (group, channel, 0x2, pack_parameter(parameter), value),
        UmpEvent::AssignableController {
            group,
            channel,
            parameter,
            value,
        } => (group, channel, 0x3, pack_parameter(parameter), value),
        UmpEvent::RelativeRegisteredController {
            group,
            channel,
            parameter,
            delta,
        } => (group, channel, 0x4, pack_parameter(parameter), delta as u32),
        UmpEvent::RelativeAssignableController {
            group,
            channel,
            parameter,
            delta,
        } => (group, channel, 0x5, pack_parameter(parameter), delta as u32),
        UmpEvent::PerNotePitchBend {
            group,
            channel,
            note,
            value,
        } => (group, channel, 0x6, u16::from(note) << 8, value),
    };
    if group > 15
        || channel > 15
        || matches!(event, UmpEvent::NoteOff { note, .. } | UmpEvent::NoteOn { note, .. } | UmpEvent::PerNotePitchBend { note, .. } if *note > 127)
        || matches!(event, UmpEvent::ControlChange { controller, .. } if *controller > 127)
        || matches!(event, UmpEvent::RegisteredController { parameter, .. } | UmpEvent::AssignableController { parameter, .. } | UmpEvent::RelativeRegisteredController { parameter, .. } | UmpEvent::RelativeAssignableController { parameter, .. } if *parameter > 16_383)
    {
        return Err(UmpError::InvalidField);
    }
    let first = (0x4u32 << 28)
        | ((group as u32) << 24)
        | ((status as u32) << 20)
        | ((channel as u32) << 16)
        | u32::from(data);
    let mut packet = [0u8; 8];
    packet[..4].copy_from_slice(&first.to_be_bytes());
    packet[4..].copy_from_slice(&value.to_be_bytes());
    Ok(packet)
}

fn pack_parameter(parameter: u16) -> u16 {
    ((parameter >> 7) << 8) | (parameter & 0x7f)
}

/// Decode one 64-bit MIDI 2.0 Channel Voice UMP packet in network byte order.
pub fn decode_channel_voice(packet: [u8; 8]) -> Result<UmpEvent, UmpError> {
    let first = u32::from_be_bytes(packet[..4].try_into().unwrap());
    let second = u32::from_be_bytes(packet[4..].try_into().unwrap());
    let message_type = (first >> 28) as u8;
    if message_type != 0x4 {
        return Err(UmpError::UnsupportedMessageType);
    }
    let group = ((first >> 24) & 0x0f) as u8;
    let status = ((first >> 20) & 0x0f) as u8;
    let channel = ((first >> 16) & 0x0f) as u8;
    let data1 = ((first >> 8) & 0xff) as u8;
    let data2 = (first & 0xff) as u8;
    if data1 > 127 || data2 > 127 {
        return Err(UmpError::InvalidField);
    }
    let parameter = (u16::from(data1) << 7) | u16::from(data2);
    match status {
        0x8 => Ok(UmpEvent::NoteOff {
            group,
            channel,
            note: data1,
            velocity: (second >> 16) as u16,
        }),
        0x9 => Ok(UmpEvent::NoteOn {
            group,
            channel,
            note: data1,
            velocity: (second >> 16) as u16,
        }),
        0xB => Ok(UmpEvent::ControlChange {
            group,
            channel,
            controller: data1,
            value: second,
        }),
        0x2 => Ok(UmpEvent::RegisteredController {
            group,
            channel,
            parameter,
            value: second,
        }),
        0x3 => Ok(UmpEvent::AssignableController {
            group,
            channel,
            parameter,
            value: second,
        }),
        0x4 => Ok(UmpEvent::RelativeRegisteredController {
            group,
            channel,
            parameter,
            delta: second as i32,
        }),
        0x5 => Ok(UmpEvent::RelativeAssignableController {
            group,
            channel,
            parameter,
            delta: second as i32,
        }),
        0x6 => Ok(UmpEvent::PerNotePitchBend {
            group,
            channel,
            note: data1,
            value: second,
        }),
        _ => Err(UmpError::InvalidField),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(status: u8, channel: u8, data1: u8, data2: u8, value: u32) -> [u8; 8] {
        let first = (0x4u32 << 28)
            | (2u32 << 24)
            | ((status as u32) << 20)
            | ((channel as u32) << 16)
            | ((data1 as u32) << 8)
            | data2 as u32;
        let mut bytes = [0; 8];
        bytes[..4].copy_from_slice(&first.to_be_bytes());
        bytes[4..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    #[test]
    fn decodes_note_on_and_full_resolution_velocity() {
        assert_eq!(
            decode_channel_voice(packet(0x9, 3, 60, 0, 0xabcd0000)),
            Ok(UmpEvent::NoteOn {
                group: 2,
                channel: 3,
                note: 60,
                velocity: 0xabcd
            })
        );
    }

    #[test]
    fn decodes_control_change_without_narrowing_value() {
        assert_eq!(
            decode_channel_voice(packet(0xB, 1, 74, 0, 0x12345678)),
            Ok(UmpEvent::ControlChange {
                group: 2,
                channel: 1,
                controller: 74,
                value: 0x12345678
            })
        );
    }

    #[test]
    fn rejects_non_channel_voice_and_unknown_status() {
        assert_eq!(
            decode_channel_voice([0; 8]),
            Err(UmpError::UnsupportedMessageType)
        );
        assert_eq!(
            decode_channel_voice(packet(0xE, 0, 0, 0, 0)),
            Err(UmpError::InvalidField)
        );
    }

    #[test]
    fn encodes_channel_voice_and_round_trips() {
        let event = UmpEvent::ControlChange {
            group: 4,
            channel: 2,
            controller: 74,
            value: 0x12345678,
        };
        let packet = encode_channel_voice(&event).unwrap();
        assert_eq!(decode_channel_voice(packet), Ok(event));
        assert!(encode_channel_voice(&UmpEvent::NoteOn {
            group: 16,
            channel: 0,
            note: 60,
            velocity: 1
        })
        .is_err());
        let bend = UmpEvent::PerNotePitchBend {
            group: 1,
            channel: 2,
            note: 60,
            value: 0x87654321,
        };
        assert_eq!(
            decode_channel_voice(encode_channel_voice(&bend).unwrap()),
            Ok(bend)
        );
    }

    #[test]
    fn round_trips_all_16384_registered_and_assignable_addresses() {
        for parameter in [0, 127, 128, 8_191, 16_383] {
            for event in [
                UmpEvent::RegisteredController {
                    group: 2,
                    channel: 3,
                    parameter,
                    value: 0x1234_5678,
                },
                UmpEvent::AssignableController {
                    group: 2,
                    channel: 3,
                    parameter,
                    value: 0x8765_4321,
                },
            ] {
                assert_eq!(
                    decode_channel_voice(encode_channel_voice(&event).unwrap()),
                    Ok(event)
                );
            }
        }
        assert!(encode_channel_voice(&UmpEvent::RegisteredController {
            group: 0,
            channel: 0,
            parameter: 16_384,
            value: 0,
        })
        .is_err());
    }

    #[test]
    fn relative_registered_and_assignable_values_remain_signed() {
        for event in [
            UmpEvent::RelativeRegisteredController {
                group: 1,
                channel: 4,
                parameter: 0x1234,
                delta: -17,
            },
            UmpEvent::RelativeAssignableController {
                group: 1,
                channel: 4,
                parameter: 0x1234,
                delta: 23,
            },
        ] {
            assert_eq!(
                decode_channel_voice(encode_channel_voice(&event).unwrap()),
                Ok(event)
            );
        }
    }

    #[test]
    fn rejects_non_seven_bit_controller_address_bytes() {
        assert_eq!(
            decode_channel_voice(packet(0x2, 0, 0x80, 0, 0)),
            Err(UmpError::InvalidField)
        );
    }
}
