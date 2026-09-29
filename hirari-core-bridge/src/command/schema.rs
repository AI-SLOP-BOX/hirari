#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct CommandDocument {
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,
    #[serde(default = "current_command_version")]
    pub command_version: u32,
    pub transaction: String,
    #[serde(default)]
    pub permission: Permission,
    #[serde(default)]
    pub expected_generation: Option<u64>,
    #[serde(default)]
    pub expected_audio_generation: Option<u64>,
    pub actions: Vec<CommandAction>,
}

fn current_schema_version() -> u32 {
    1
}
fn current_command_version() -> u32 {
    1
}

fn default_one() -> f32 {
    1.0
}

fn default_stem_tail_seconds() -> f32 {
    2.0
}

fn default_include_inserts() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    #[default]
    ReadOnly,
    ProjectWrite,
    SystemWrite,
    /// Explicit opt-in for power users and trusted local automation. This
    /// bypasses project-root path containment but never bypasses request
    /// logging, generation checks, or transaction/idempotency requirements.
    Unrestricted,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PathPolicy {
    ProjectOnly,
    Unrestricted,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MutationClass {
    ReadOnly,
    Reversible,
    Irreversible,
    ExternalSideEffect,
}


