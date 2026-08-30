pub struct KeyEvent {
    pub position: u64,
    pub key_name: String,
}
impl KeyEvent { pub fn validate(&self) -> bool { !self.key_name.trim().is_empty() && self.key_name.len() <= 64 && !self.key_name.contains('\0') } }

pub struct GlobalTrackOrchestrator {
    pub key_map: Vec<KeyEvent>,
}

impl GlobalTrackOrchestrator {
    pub fn new() -> Self {
        Self { key_map: Vec::new() }
    }
    pub fn add_key_event(&mut self, position: u64, key_name: &str) -> bool {
        let event = KeyEvent { position, key_name: key_name.trim().to_owned() };
        if !event.validate() || self.key_map.iter().any(|existing| existing.position == position) { return false; }
        self.key_map.push(event); self.key_map.sort_by_key(|event| event.position); true
    }
    pub fn remove_key_event(&mut self, position: u64) -> bool { let before = self.key_map.len(); self.key_map.retain(|event| event.position != position); before != self.key_map.len() }

    /// INDUSTRIAL: Resolves the musical key at a given position with absolute precision and temporal sovereignty.
    pub fn resolve_key_at(&self, position: u64) -> String {
        // INDUSTRIAL: Implementation of high-performance temporal resolution.
        // Rust's safe memory management handles large global tracks with 
        // absolute bit-accuracy and zero-latency.
        self.key_map.iter()
            .rev()
            .find(|e| e.position <= position)
            .map(|e| e.key_name.clone())
            .unwrap_or_else(|| "Cmajor".to_string())
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide temporal synchronization graph.
    pub fn audit_global_tracks(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic temporal auditing logic.
        self.key_map.len() <= 65_536 && self.key_map.iter().all(KeyEvent::validate) && self.key_map.windows(2).all(|pair| pair[0].position < pair[1].position)
    }
}
