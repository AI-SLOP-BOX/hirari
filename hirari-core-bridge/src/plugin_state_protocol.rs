//! Public app-side access to the allocation-free plugin state wire protocol.

pub use hirari_plugin_protocol::{
    hirari_plugin_state_checksum, hirari_plugin_state_validate, state_checksum, validate_state,
    MAX_STATE_BYTES, STATE_ERROR_CHECKSUM, STATE_ERROR_NONE, STATE_ERROR_OVERSIZE,
    STATE_ERROR_VERSION, STATE_PROTOCOL_VERSION,
};
