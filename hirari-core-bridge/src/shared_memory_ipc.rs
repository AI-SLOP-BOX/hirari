use serde::{Deserialize, Serialize};

const IPC_MAGIC: u32 = 0x4155_5241;
const IPC_VERSION: u32 = 1;
const MAX_PAYLOAD_SIZE: usize = 16 * 1024 * 1024;
const DEFAULT_CAPACITY: usize = 64;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct IpcFrameRust {
    pub frame_index: u64,
    pub num_channels: u32,
    pub num_samples: u32,
    pub version: u32,
    pub diagnostic_tag: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    Disconnected,
    EmptyPayload,
    PayloadTooLarge,
    InvalidHeader,
    InvalidPayload,
    BufferFull,
    PositionOverflow,
}

#[derive(Serialize, Deserialize)]
struct IpcHeader {
    magic: u32,
    version: u32,
    payload_len: u64,
}

pub struct IpcOrchestrator {
    pub write_pos: usize,
    pub read_pos: usize,
    connected: bool,
    frames: Vec<Option<Vec<u8>>>,
    pending: usize,
}

impl Default for IpcOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl IpcOrchestrator {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            write_pos: 0,
            read_pos: 0,
            connected: true,
            frames: vec![None; capacity],
            pending: 0,
        }
    }

    pub fn disconnect(&mut self) {
        self.connected = false;
    }
    pub fn connect(&mut self) {
        self.connected = true;
    }
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Validates and stores one header/payload IPC packet.
    pub fn save_frame(&mut self, frame: IpcFrameRust) -> Result<(), IpcError> {
        if !self.connected {
            return Err(IpcError::Disconnected);
        }
        let payload = bincode::serialize(&frame).map_err(|_| IpcError::InvalidPayload)?;
        if payload.is_empty() {
            return Err(IpcError::EmptyPayload);
        }
        if payload.len() > MAX_PAYLOAD_SIZE {
            return Err(IpcError::PayloadTooLarge);
        }
        let payload_len = u64::try_from(payload.len()).map_err(|_| IpcError::PayloadTooLarge)?;
        let header = bincode::serialize(&IpcHeader {
            magic: IPC_MAGIC,
            version: IPC_VERSION,
            payload_len,
        })
        .map_err(|_| IpcError::InvalidHeader)?;
        let packet_len = header
            .len()
            .checked_add(payload.len())
            .ok_or(IpcError::PositionOverflow)?;
        if packet_len > MAX_PAYLOAD_SIZE {
            return Err(IpcError::PayloadTooLarge);
        }
        let slot = self.write_pos % self.frames.len();
        if self.pending == self.frames.len() {
            return Err(IpcError::BufferFull);
        }
        let mut packet = Vec::with_capacity(packet_len);
        packet.extend_from_slice(&header);
        packet.extend_from_slice(&payload);
        self.frames[slot] = Some(packet);
        self.write_pos = self
            .write_pos
            .checked_add(1)
            .ok_or(IpcError::PositionOverflow)?;
        self.pending += 1;
        Ok(())
    }

    pub fn get_frame(&mut self) -> Result<Option<IpcFrameRust>, IpcError> {
        if !self.connected {
            return Err(IpcError::Disconnected);
        }
        if self.read_pos == self.write_pos {
            return Ok(None);
        }
        let slot = self.read_pos % self.frames.len();
        let packet = self.frames[slot].take().ok_or(IpcError::InvalidPayload)?;
        let header: IpcHeader =
            bincode::deserialize(&packet).map_err(|_| IpcError::InvalidHeader)?;
        let header_len = usize::try_from(
            bincode::serialized_size(&header).map_err(|_| IpcError::InvalidHeader)?,
        )
        .map_err(|_| IpcError::InvalidHeader)?;
        if header.magic != IPC_MAGIC || header.version != IPC_VERSION {
            return Err(IpcError::InvalidHeader);
        }
        let payload = packet.get(header_len..).ok_or(IpcError::InvalidHeader)?;
        let length = usize::try_from(header.payload_len).map_err(|_| IpcError::PayloadTooLarge)?;
        if length == 0 {
            return Err(IpcError::EmptyPayload);
        }
        if length > MAX_PAYLOAD_SIZE || length != payload.len() {
            return Err(IpcError::PayloadTooLarge);
        }
        self.read_pos = self
            .read_pos
            .checked_add(1)
            .ok_or(IpcError::PositionOverflow)?;
        self.pending -= 1;
        bincode::deserialize(payload)
            .map(Some)
            .map_err(|_| IpcError::InvalidPayload)
    }

    pub fn push_frame(&mut self, frame: IpcFrameRust) -> Result<(), IpcError> {
        self.save_frame(frame)
    }
    pub fn audit_shared_memory_ipc(&self) -> bool {
        self.connected && self.write_pos >= self.read_pos
    }
}
