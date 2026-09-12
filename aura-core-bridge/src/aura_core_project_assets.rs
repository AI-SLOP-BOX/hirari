impl AuraCore {
    pub fn scan_project_assets(&self, project_dir: &str, assets: Vec<String>) -> Vec<String> {
        SovereignPersistence::scan_assets(project_dir, &assets)
    }

    pub fn get_region_waveform(&self, tid: u32, rid: u32) -> Vec<f32> {
        self.engine.as_ref().map_or_else(Vec::new, |e| {
            e.get_region_waveform(tid, rid).into_iter().collect()
        })
    }
    pub fn queue_region_waveform(&self, tid: u32, rid: u32) -> u64 {
        self.engine.as_ref().map_or(0, |e| e.queue_region_waveform(tid, rid))
    }
    pub fn poll_region_waveform(&self, request: u64) -> Vec<f32> {
        self.engine.as_ref().map_or_else(Vec::new, |e| {
            e.poll_region_waveform(request).into_iter().collect()
        })
    }
    pub fn region_waveform_pending(&self, request: u64) -> bool {
        self.engine.as_ref().is_some_and(|e| e.region_waveform_pending(request))
    }
    pub fn get_track_correlation(&self, tid: u32) -> f32 {
        self.engine
            .as_ref()
            .map_or(0.0, |e| e.get_track_correlation(tid))
    }
    pub fn get_project_layout_json(&self) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return String::new();
        };
        let raw = engine.get_project_layout_json();
        let Ok(mut layout) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return raw;
        };
        let Ok(aux_ids) = serde_json::from_str::<Vec<u32>>(&self.aux_track_ids_json()) else {
            return raw;
        };
        let Some(tracks) = layout.as_array_mut() else {
            return raw;
        };
        for track in tracks {
            let Some(id) = track.get("id").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            if aux_ids.contains(&(id as u32))
                && track.get("type").and_then(serde_json::Value::as_str) == Some("Bus")
            {
                track["type"] = serde_json::Value::String("Aux".to_owned());
            }
        }
        serde_json::to_string(&layout).unwrap_or(raw)
    }

    /// Generation of the exact layout snapshot returned above.  Commands
    /// must echo this value back when applying a mutation, preventing a UI or
    /// external harness from editing a newer project from an old snapshot.
    pub fn project_generation(&self) -> u64 {
        crate::command_api::snapshot_generation(self.get_project_layout_json().as_bytes())
    }
}
