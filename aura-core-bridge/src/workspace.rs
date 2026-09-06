use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceLayout {
    pub name: String,
    pub windows: Vec<WorkspaceWindow>,
    pub scale: f32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceWindow {
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub visible: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceRegistry {
    pub layouts: Vec<WorkspaceLayout>,
    pub active: Option<String>,
}
impl WorkspaceRegistry {
    pub fn layout_names(&self) -> Vec<String> {
        self.layouts.iter().map(|l| l.name.clone()).collect()
    }
    pub fn upsert(&mut self, layout: WorkspaceLayout) -> bool {
        if !layout.validate() {
            return false;
        }
        if let Some(existing) = self
            .layouts
            .iter_mut()
            .find(|existing| existing.name.eq_ignore_ascii_case(&layout.name))
        {
            *existing = layout;
        } else if self.layouts.len() < 256 {
            self.layouts.push(layout);
        } else {
            return false;
        }
        self.layouts
            .sort_by_key(|layout| layout.name.to_ascii_lowercase());
        true
    }
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.layouts.len();
        self.layouts
            .retain(|l| !l.name.eq_ignore_ascii_case(name.trim()));
        if self
            .active
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case(name.trim()))
        {
            self.active = None;
        }
        before != self.layouts.len()
    }
    pub fn activate(&mut self, name: &str) -> bool {
        if self
            .layouts
            .iter()
            .any(|l| l.name.eq_ignore_ascii_case(name.trim()))
        {
            self.active = Some(name.trim().to_owned());
            true
        } else {
            false
        }
    }
    pub fn get(&self, name: &str) -> Option<&WorkspaceLayout> {
        self.layouts
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(name.trim()))
    }
    pub fn audit(&self) -> bool {
        self.layouts.len() <= 256
            && self.layouts.iter().all(WorkspaceLayout::validate)
            && self
                .layouts
                .windows(2)
                .all(|w| w[0].name.to_ascii_lowercase() < w[1].name.to_ascii_lowercase())
            && self.active.as_deref().is_none_or(|a| self.get(a).is_some())
    }
}

impl WorkspaceLayout {
    pub fn validate(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 128
            && self.windows.len() <= 256
            && self.scale.is_finite()
            && (0.25..=4.0).contains(&self.scale)
            && self.windows.iter().all(|w| {
                !w.id.trim().is_empty()
                    && w.id.len() <= 128
                    && w.width > 0
                    && w.height > 0
                    && w.width <= 16384
                    && w.height <= 16384
            })
            && self
                .windows
                .iter()
                .enumerate()
                .all(|(i, w)| self.windows[..i].iter().all(|p| p.id != w.id))
    }
    pub fn upsert_window(&mut self, window: WorkspaceWindow) -> bool {
        if window.id.trim().is_empty()
            || window.id.len() > 128
            || window.id.contains('\0')
            || window.width == 0
            || window.height == 0
            || window.width > 16384
            || window.height > 16384
        {
            return false;
        }
        if let Some(existing) = self
            .windows
            .iter_mut()
            .find(|existing| existing.id == window.id)
        {
            *existing = window;
        } else if self.windows.len() < 256 {
            self.windows.push(window);
        } else {
            return false;
        }
        self.validate()
    }
    pub fn remove_window(&mut self, id: &str) -> bool {
        let before = self.windows.len();
        self.windows.retain(|w| w.id != id);
        before != self.windows.len()
    }
    pub fn set_visible(&mut self, id: &str, visible: bool) -> bool {
        self.windows
            .iter_mut()
            .find(|w| w.id == id)
            .map(|w| {
                w.visible = visible;
                true
            })
            .unwrap_or(false)
    }
    pub fn to_json(&self) -> Result<String, String> {
        if !self.validate() {
            return Err("invalid workspace layout".into());
        }
        let mut normalized = self.clone();
        normalized.windows.sort_by(|a, b| a.id.cmp(&b.id));
        serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())
    }
    pub fn from_json(json: &str) -> Result<Self, String> {
        let layout: Self =
            serde_json::from_str(json).map_err(|e| format!("invalid workspace JSON: {e}"))?;
        if !layout.validate() {
            return Err("invalid workspace layout".into());
        }
        Ok(layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_layout_bounds() {
        let l = WorkspaceLayout {
            name: "main".into(),
            windows: vec![WorkspaceWindow {
                id: "arrange".into(),
                x: 0,
                y: 0,
                width: 800,
                height: 600,
                visible: true,
            }],
            scale: 1.0,
        };
        assert!(l.validate());
    }
    #[test]
    fn workspace_json_roundtrip_rejects_invalid_layouts() {
        let l = WorkspaceLayout {
            name: "main".into(),
            windows: vec![],
            scale: 1.0,
        };
        let json = l.to_json().unwrap();
        assert_eq!(WorkspaceLayout::from_json(&json).unwrap(), l);
        assert!(WorkspaceLayout::from_json(r#"{"name":"","windows":[],"scale":1.0}"#).is_err());
    }
}
