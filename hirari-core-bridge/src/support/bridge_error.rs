use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable control-plane error information shared by CLI, UI, and future FFI
/// adapters. Existing bool/Result APIs remain compatible; new boundaries can
/// return this value without losing retry or generation context.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_object: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

impl BridgeError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            affected_object: None,
            generation: None,
        }
    }

    pub fn retryable(mut self, value: bool) -> Self {
        self.retryable = value;
        self
    }

    pub fn object(mut self, value: impl Into<String>) -> Self {
        self.affected_object = Some(value.into());
        self
    }

    pub fn at_generation(mut self, value: u64) -> Self {
        self.generation = Some(value);
        self
    }

    pub fn generation(self, value: u64) -> Self {
        self.at_generation(value)
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BridgeError {}
