use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShortcutBinding {
    pub key: String,
    pub command: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandMacro {
    pub name: String,
    pub commands: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MacroRegistry {
    pub bindings: Vec<ShortcutBinding>,
    pub macros: Vec<CommandMacro>,
}

impl MacroRegistry {
    pub fn upsert_binding(&mut self, binding: ShortcutBinding) -> bool {
        if !binding.validate() {
            return false;
        }
        if let Some(existing) = self
            .bindings
            .iter_mut()
            .find(|b| b.key.trim().eq_ignore_ascii_case(binding.key.trim()))
        {
            *existing = binding;
        } else {
            self.bindings.push(binding);
        }
        validate_shortcut_bindings(&self.bindings)
    }
    pub fn remove_binding(&mut self, key: &str) -> bool {
        let before = self.bindings.len();
        self.bindings
            .retain(|b| !b.key.trim().eq_ignore_ascii_case(key.trim()));
        before != self.bindings.len()
    }
    pub fn upsert_macro(&mut self, macro_def: CommandMacro) -> bool {
        if !macro_def.validate() {
            return false;
        }
        if let Some(existing) = self
            .macros
            .iter_mut()
            .find(|m| m.name.trim().eq_ignore_ascii_case(macro_def.name.trim()))
        {
            *existing = macro_def;
        } else {
            self.macros.push(macro_def);
        }
        self.macros.sort_by_key(|m| m.name.to_ascii_lowercase());
        self.macros.len() <= 4096 && self.macros.iter().all(CommandMacro::validate)
    }
    pub fn macro_names(&self) -> Vec<String> {
        self.macros.iter().map(|m| m.name.clone()).collect()
    }
    pub fn expand(&self, name: &str) -> Option<Vec<String>> {
        self.macros
            .iter()
            .find(|m| m.name.trim().eq_ignore_ascii_case(name.trim()))
            .map(CommandMacro::expanded)
    }
    pub fn audit(&self) -> bool {
        validate_shortcut_bindings(&self.bindings)
            && self.macros.len() <= 4096
            && self.macros.iter().all(CommandMacro::validate)
            && self
                .macros
                .windows(2)
                .all(|w| w[0].name.to_ascii_lowercase() < w[1].name.to_ascii_lowercase())
    }
}

impl ShortcutBinding {
    pub fn validate(&self) -> bool {
        valid_token(&self.key, 64) && valid_token(&self.command, 128)
    }
}
impl CommandMacro {
    pub fn validate(&self) -> bool {
        valid_token(&self.name, 128)
            && !self.commands.is_empty()
            && self.commands.len() <= 64
            && self.commands.iter().all(|c| valid_token(c, 128))
    }
    pub fn expanded(&self) -> Vec<String> {
        self.commands.clone()
    }
}
/// Validates a shortcut map as a whole, including collision detection.
pub fn validate_shortcut_bindings(bindings: &[ShortcutBinding]) -> bool {
    bindings.len() <= 4096
        && bindings.iter().all(ShortcutBinding::validate)
        && bindings.iter().enumerate().all(|(i, binding)| {
            bindings[..i]
                .iter()
                .all(|previous| previous.key.trim() != binding.key.trim())
        })
}
fn valid_token(value: &str, max: usize) -> bool {
    let t = value.trim();
    !t.is_empty() && t.len() <= max && !t.bytes().any(|b| b == 0 || b == b'\n' || b == b'\r')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_macro_chain() {
        let m = CommandMacro {
            name: "mix".into(),
            commands: vec!["save".into(), "bounce".into()],
        };
        assert!(m.validate());
        assert_eq!(m.expanded().len(), 2);
    }
    #[test]
    fn rejects_shortcut_collisions() {
        let ok = vec![ShortcutBinding {
            key: "Cmd+S".into(),
            command: "save".into(),
        }];
        assert!(validate_shortcut_bindings(&ok));
        let dup = vec![
            ShortcutBinding {
                key: "Cmd+S".into(),
                command: "save".into(),
            },
            ShortcutBinding {
                key: " Cmd+S ".into(),
                command: "save_as".into(),
            },
        ];
        assert!(!validate_shortcut_bindings(&dup));
    }
}
