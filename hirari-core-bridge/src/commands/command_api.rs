use crate::bridge_error::BridgeError;
use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, read};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

include!("command/protocol.rs");
include!("command/schema.rs");
include!("command/actions.rs");
include!("command/validation.rs");
include!("command/validation_rules.rs");
include!("command/paths.rs");
include!("command/ledger.rs");
include!("command/diagnostics.rs");
include!("command/tests.rs");
