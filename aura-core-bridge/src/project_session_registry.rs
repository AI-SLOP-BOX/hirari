//! Bounded project-session registry for multi-project control clients.
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub struct ProjectSessionRegistry {
    sessions: BTreeMap<String, String>,
    active: Option<String>,
}

impl ProjectSessionRegistry {
    pub fn open(
        &mut self,
        id: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<(), &'static str> {
        let id = id.into().trim().to_owned();
        let path = path.into().trim().to_owned();
        if id.is_empty()
            || id.len() > 128
            || id.contains('\0')
            || path.is_empty()
            || path.len() > 4096
            || path.contains('\0')
        {
            return Err("invalid project session");
        }
        if self.sessions.len() >= 32 && !self.sessions.contains_key(&id) {
            return Err("project session limit reached");
        }
        self.sessions.insert(id.clone(), path);
        self.active.get_or_insert(id);
        Ok(())
    }
    pub fn activate(&mut self, id: &str) -> bool {
        if self.sessions.contains_key(id) {
            self.active = Some(id.to_owned());
            true
        } else {
            false
        }
    }
    pub fn close(&mut self, id: &str) -> bool {
        let removed = self.sessions.remove(id).is_some();
        if self.active.as_deref() == Some(id) {
            self.active = self.sessions.keys().next().cloned();
        }
        removed
    }
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }
    pub fn len(&self) -> usize {
        self.sessions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
    pub fn path(&self, id: &str) -> Option<&str> {
        self.sessions.get(id).map(String::as_str)
    }
    pub fn sessions(&self) -> Vec<(String, String)> {
        self.sessions
            .iter()
            .map(|(id, path)| (id.clone(), path.clone()))
            .collect()
    }
    pub fn audit(&self) -> bool {
        self.sessions.len() <= 32
            && self.sessions.iter().all(|(id, path)| {
                !id.trim().is_empty()
                    && id.len() <= 128
                    && !id.contains('\0')
                    && !path.trim().is_empty()
                    && path.len() <= 4096
                    && !path.contains('\0')
            })
            && self
                .active
                .as_deref()
                .is_none_or(|id| self.sessions.contains_key(id))
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectSessionRegistry;
    #[test]
    fn switches_independent_project_sessions_with_bounded_capacity() {
        let mut registry = ProjectSessionRegistry::default();
        registry.open("one", "/tmp/one.aura").unwrap();
        registry.open("two", "/tmp/two.aura").unwrap();
        assert_eq!(registry.active(), Some("one"));
        assert!(registry.activate("two"));
        assert_eq!(registry.active(), Some("two"));
        assert!(registry.close("two"));
        assert_eq!(registry.active(), Some("one"));
        assert!(!registry.activate("missing"));
        assert_eq!(registry.path("one"), Some("/tmp/one.aura"));
        assert_eq!(registry.sessions().len(), 1);
        assert!(registry.audit());
    }
}
