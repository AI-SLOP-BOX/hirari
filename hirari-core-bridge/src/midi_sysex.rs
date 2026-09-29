//! Bounded SysEx fragmentation/reassembly for control-plane and MIDI bridge
//! callers.  The legacy realtime mailbox still rejects payloads larger than
//! its fixed event slot; this module provides the lossless packet format that
//! higher layers can use before handing events to a transport with a larger
//! payload budget.

pub const MAX_FRAGMENT_PAYLOAD: usize = 240;
pub const MAX_SYSEX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UmpError {
    EmptyPacket,
    UnsupportedMessageType,
    InvalidWordCount,
    GroupOutOfRange,
}

/// Validate a MIDI 2.0 Universal MIDI Packet without interpreting musical
/// values. The first word carries the message type and group; packet length
/// is determined by the message type, so malformed payloads are rejected
/// before they can enter a fixed-size realtime mailbox.
pub fn validate_ump_packet(words: &[u32]) -> Result<(), UmpError> {
    let Some(&first) = words.first() else {
        return Err(UmpError::EmptyPacket);
    };
    let message_type = (first >> 28) as u8;
    let group = ((first >> 24) & 0x0f) as u8;
    if group > 15 {
        return Err(UmpError::GroupOutOfRange);
    }
    let expected_words = match message_type {
        0x0..=0x2 => 1,
        0x3..=0x4 => 2,
        0x5..=0x7 => 4,
        _ => return Err(UmpError::UnsupportedMessageType),
    };
    if words.len() != expected_words {
        return Err(UmpError::InvalidWordCount);
    }
    Ok(())
}

/// One MIDI 2.0 SysEx8 data-message packet. This bridge uses the conservative
/// twelve-byte payload layout after the four-byte control header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sysex8Packet {
    pub group: u8,
    /// 0=start, 1=continue, 2=end, 3=complete.
    pub status: u8,
    pub stream_id: u8,
    pub payload: Vec<u8>,
}

pub fn encode_sysex8(packet: &Sysex8Packet) -> Result<[u32; 4], UmpError> {
    if packet.group > 15
        || packet.status > 3
        || packet.payload.is_empty()
        || packet.payload.len() > 12
    {
        return Err(UmpError::InvalidWordCount);
    }
    let first = (0x5u32 << 28)
        | ((packet.group as u32) << 24)
        | ((packet.status as u32) << 20)
        | ((packet.payload.len() as u32) << 16)
        | ((packet.stream_id as u32) << 8);
    let mut bytes = [0u8; 16];
    bytes[..4].copy_from_slice(&first.to_be_bytes());
    for (dst, src) in bytes[3..].iter_mut().skip(1).zip(packet.payload.iter()) {
        *dst = *src;
    }
    Ok([
        u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
        u32::from_be_bytes(bytes[4..8].try_into().unwrap()),
        u32::from_be_bytes(bytes[8..12].try_into().unwrap()),
        u32::from_be_bytes(bytes[12..16].try_into().unwrap()),
    ])
}

pub fn decode_sysex8(words: [u32; 4]) -> Result<Sysex8Packet, UmpError> {
    validate_ump_packet(&words)?;
    let bytes = words
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect::<Vec<_>>();
    let length = ((words[0] >> 16) & 0x0f) as usize;
    if length == 0 || length > 12 {
        return Err(UmpError::InvalidWordCount);
    }
    Ok(Sysex8Packet {
        group: ((words[0] >> 24) & 0x0f) as u8,
        status: ((words[0] >> 20) & 0x0f) as u8,
        stream_id: ((words[0] >> 8) & 0xff) as u8,
        payload: bytes[4..4 + length].to_vec(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SysexFragment {
    pub message_id: u32,
    pub index: u16,
    pub total: u16,
    pub start: bool,
    pub end: bool,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SysexError {
    EmptyMessage,
    TooLarge,
    TooManyFragments,
    InvalidFragment,
    OutOfOrder,
    Incomplete,
}

pub fn fragment(message_id: u32, bytes: &[u8]) -> Result<Vec<SysexFragment>, SysexError> {
    if bytes.is_empty() {
        return Err(SysexError::EmptyMessage);
    }
    if bytes.len() > MAX_SYSEX_BYTES {
        return Err(SysexError::TooLarge);
    }
    let count = bytes.len().div_ceil(MAX_FRAGMENT_PAYLOAD);
    if count > u16::MAX as usize {
        return Err(SysexError::TooManyFragments);
    }
    Ok(bytes
        .chunks(MAX_FRAGMENT_PAYLOAD)
        .enumerate()
        .map(|(index, payload)| SysexFragment {
            message_id,
            index: index as u16,
            total: count as u16,
            start: index == 0,
            end: index + 1 == count,
            payload: payload.to_vec(),
        })
        .collect())
}

#[derive(Default)]
pub struct SysexReassembler {
    message_id: Option<u32>,
    next_index: u16,
    total: u16,
    bytes: Vec<u8>,
}

impl SysexReassembler {
    pub fn reset(&mut self) {
        self.message_id = None;
        self.next_index = 0;
        self.total = 0;
        self.bytes.clear();
    }

    pub fn push(&mut self, fragment: SysexFragment) -> Result<Option<Vec<u8>>, SysexError> {
        let valid = fragment.total > 0
            && fragment.index < fragment.total
            && !fragment.payload.is_empty()
            && fragment.payload.len() <= MAX_FRAGMENT_PAYLOAD
            && fragment.start == (fragment.index == 0)
            && fragment.end == (fragment.index + 1 == fragment.total);
        if !valid {
            self.reset();
            return Err(SysexError::InvalidFragment);
        }
        if fragment.index == 0 {
            self.reset();
            self.message_id = Some(fragment.message_id);
            self.total = fragment.total;
        }
        if self.message_id != Some(fragment.message_id)
            || self.total != fragment.total
            || fragment.index != self.next_index
        {
            self.reset();
            return Err(SysexError::OutOfOrder);
        }
        if self.bytes.len() + fragment.payload.len() > MAX_SYSEX_BYTES {
            self.reset();
            return Err(SysexError::TooLarge);
        }
        self.bytes.extend_from_slice(&fragment.payload);
        self.next_index += 1;
        if fragment.end {
            let result = std::mem::take(&mut self.bytes);
            self.reset();
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_large_sysex_in_order() {
        let input: Vec<u8> = (0..=255).cycle().take(1025).collect();
        let fragments = fragment(7, &input).unwrap();
        assert!(fragments.len() > 4);
        let mut reassembler = SysexReassembler::default();
        let mut result = None;
        for packet in fragments {
            result = reassembler.push(packet).unwrap();
        }
        assert_eq!(result, Some(input));
    }

    #[test]
    fn rejects_reordering_and_resets_state() {
        let mut packets = fragment(9, &[1; 500]).unwrap();
        let second = packets.remove(1);
        let mut reassembler = SysexReassembler::default();
        assert!(reassembler.push(second).is_err());
        assert!(reassembler.push(packets.remove(0)).unwrap().is_none());
    }

    #[test]
    fn rejects_empty_and_oversize_messages() {
        assert_eq!(fragment(1, &[]), Err(SysexError::EmptyMessage));
        assert_eq!(
            fragment(1, &vec![0; MAX_SYSEX_BYTES + 1]),
            Err(SysexError::TooLarge)
        );
    }

    #[test]
    fn validates_ump_packet_lengths_and_groups() {
        assert!(validate_ump_packet(&[0x4012_3456, 0x789a_bcde]).is_ok());
        assert!(validate_ump_packet(&[0x4012_3456]).is_err());
        assert!(validate_ump_packet(&[0x7012_3456, 0, 0, 0]).is_ok());
        assert_eq!(
            validate_ump_packet(&[0xF012_3456]),
            Err(UmpError::UnsupportedMessageType)
        );
    }

    #[test]
    fn rejects_invalid_ump_word_count_before_transport() {
        assert_eq!(validate_ump_packet(&[]), Err(UmpError::EmptyPacket));
        assert_eq!(
            validate_ump_packet(&[0x4012_3456, 0, 0]),
            Err(UmpError::InvalidWordCount)
        );
    }

    #[test]
    fn sysex8_round_trips_complete_packet() {
        let packet = Sysex8Packet {
            group: 2,
            status: 3,
            stream_id: 9,
            payload: (1..=12).collect(),
        };
        assert_eq!(
            decode_sysex8(encode_sysex8(&packet).unwrap()).unwrap(),
            packet
        );
        assert!(validate_ump_packet(&[0x530c_0900, 0, 0, 0]).is_ok());
    }
}
