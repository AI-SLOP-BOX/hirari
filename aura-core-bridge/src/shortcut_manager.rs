use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShortcutBinding { pub command: String, pub key: String, pub context: String }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandMacro { pub name: String, pub commands: Vec<String> }

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShortcutManager { pub bindings: Vec<ShortcutBinding>, pub macros: Vec<CommandMacro> }

impl ShortcutManager {
    pub fn bind(&mut self, binding: ShortcutBinding) -> bool {
        if !valid_text(&binding.command, 128) || !valid_text(&binding.key, 64) || binding.context.len() > 128 || binding.context.contains('\0') { return false; }
        if let Some(existing) = self.bindings.iter_mut().find(|b| b.key.eq_ignore_ascii_case(&binding.key) && b.context.eq_ignore_ascii_case(&binding.context)) { *existing = binding; }
        else if self.bindings.len() < 16_384 { self.bindings.push(binding); } else { return false; }
        true
    }
    pub fn unbind(&mut self, key: &str, context: &str) -> bool { let before = self.bindings.len(); self.bindings.retain(|b| !(b.key.eq_ignore_ascii_case(key.trim()) && b.context.eq_ignore_ascii_case(context.trim()))); before != self.bindings.len() }
    pub fn resolve(&self, key: &str, context: &str) -> Option<&str> { self.bindings.iter().find(|b| b.key.eq_ignore_ascii_case(key.trim()) && (b.context.eq_ignore_ascii_case(context.trim()) || b.context.is_empty())).map(|b| b.command.as_str()) }
    pub fn define_macro(&mut self, macro_def: CommandMacro) -> bool {
        if !valid_text(&macro_def.name, 128) || macro_def.commands.is_empty() || macro_def.commands.len() > 256 || macro_def.commands.iter().any(|c| !valid_text(c, 128)) { return false; }
        if let Some(existing) = self.macros.iter_mut().find(|m| m.name.eq_ignore_ascii_case(&macro_def.name)) { *existing = macro_def; } else if self.macros.len() < 4096 { self.macros.push(macro_def); } else { return false; }
        true
    }
    pub fn expand_macro(&self, name: &str) -> Option<&[String]> { self.macros.iter().find(|m| m.name.eq_ignore_ascii_case(name.trim())).map(|m| m.commands.as_slice()) }
    /// Expands nested macro references deterministically while rejecting
    /// recursive cycles and unbounded command growth.
    pub fn expand_macro_chain(&self, name: &str) -> Option<Vec<String>> {
        let mut active = std::collections::HashSet::new();
        let mut out = Vec::new();
        self.expand_macro_inner(name.trim(), &mut active, &mut out)?;
        Some(out)
    }
    fn expand_macro_inner(&self, name: &str, active: &mut std::collections::HashSet<String>, out: &mut Vec<String>) -> Option<()> {
        let key = name.to_ascii_lowercase();
        let commands = self.expand_macro(name)?;
        if !active.insert(key.clone()) { return None; }
        for command in commands {
            if self.macros.iter().any(|macro_def| macro_def.name.eq_ignore_ascii_case(command.trim())) {
                self.expand_macro_inner(command, active, out)?;
            } else {
                if out.len() >= 16_384 { active.remove(&key); return None; }
                out.push(command.clone());
            }
        }
        active.remove(&key);
        Some(())
    }
    pub fn search_commands(&self, query: &str) -> Vec<&str> { let q = query.trim().to_ascii_lowercase(); let mut out: Vec<_> = self.bindings.iter().filter(|b| q.is_empty() || b.command.to_ascii_lowercase().contains(&q)).map(|b| b.command.as_str()).collect(); out.sort_unstable(); out.dedup(); out }
    pub fn audit(&self) -> bool { self.bindings.len() <= 16_384 && self.macros.len() <= 4096 && self.bindings.iter().all(|b| valid_text(&b.command,128) && valid_text(&b.key,64) && b.context.len() <= 128 && !b.context.contains('\0')) && self.bindings.iter().enumerate().all(|(i,b)| self.bindings[..i].iter().all(|previous| !(previous.key.eq_ignore_ascii_case(&b.key) && previous.context.eq_ignore_ascii_case(&b.context)))) && self.macros.iter().all(|m| valid_text(&m.name,128) && !m.commands.is_empty() && m.commands.len() <= 256 && m.commands.iter().all(|c| valid_text(c,128))) && self.macros.iter().enumerate().all(|(i,m)| self.macros[..i].iter().all(|previous| !previous.name.eq_ignore_ascii_case(&m.name))) }
}

fn valid_text(value: &str, max: usize) -> bool { !value.trim().is_empty() && value.len() <= max && !value.contains('\0') }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn resolves_context_and_macro() { let mut m=ShortcutManager::default(); assert!(m.bind(ShortcutBinding{command:"Play".into(),key:"Space".into(),context:"arrange".into()})); assert_eq!(m.resolve("space","ARRANGE"),Some("Play")); assert!(m.define_macro(CommandMacro{name:"Mix".into(),commands:vec!["Save".into(),"Export".into()]})); assert_eq!(m.expand_macro("mix").unwrap().len(),2); assert!(m.audit()); }
    #[test] fn nested_macros_expand_and_cycles_are_rejected() { let mut m=ShortcutManager::default(); assert!(m.define_macro(CommandMacro{name:"Render".into(),commands:vec!["Save".into(),"Print".into()]})); assert!(m.define_macro(CommandMacro{name:"Publish".into(),commands:vec!["Render".into(),"Upload".into()]})); assert_eq!(m.expand_macro_chain("publish").unwrap(), vec!["Save", "Print", "Upload"]); assert!(m.define_macro(CommandMacro{name:"Loop".into(),commands:vec!["Loop".into()]})); assert!(m.expand_macro_chain("loop").is_none()); }
}
