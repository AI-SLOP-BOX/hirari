impl HirariCore {
    /// Requests native project-layout serialization on an engine-owned
    /// background thread. The CXX AudioEngine handle never leaves its owner
    /// thread; only the native engine's shared lifetime handle is captured.
    pub fn request_project_layout_snapshot(&self) -> u64 {
        self.engine
            .as_ref()
            .map_or(0, |engine| engine.request_project_layout_snapshot())
    }

    /// Returns an empty string while the layout snapshot is being prepared,
    /// otherwise `READY <revision>\n<layout-json>` or a failure marker.
    pub fn poll_project_layout_snapshot(&self, request_id: u64) -> String {
        self.engine.as_ref().map_or_else(
            String::new,
            |engine| engine.poll_project_layout_snapshot(request_id).to_string(),
        )
    }

    pub fn cancel_project_layout_snapshot(&self, request_id: u64) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.cancel_project_layout_snapshot(request_id))
    }

    pub fn scan_project_assets(&self, project_dir: &str, assets: Vec<String>) -> Vec<String> {
        SovereignPersistence::scan_assets(project_dir, &assets)
    }

    pub fn get_region_waveform(&self, tid: u32, rid: u32) -> Vec<f32> {
        self.engine.as_ref().map_or_else(Vec::new, |e| {
            e.get_region_waveform(tid, rid).into_iter().collect()
        })
    }
    pub fn queue_region_waveform(&self, tid: u32, rid: u32) -> u64 {
        self.engine
            .as_ref()
            .map_or(0, |e| e.queue_region_waveform(tid, rid))
    }
    pub fn poll_region_waveform(&self, request: u64) -> Vec<f32> {
        self.engine.as_ref().map_or_else(Vec::new, |e| {
            e.poll_region_waveform(request).into_iter().collect()
        })
    }
    pub fn region_waveform_pending(&self, request: u64) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.region_waveform_pending(request))
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
        Self::normalize_project_layout_json(raw, &self.aux_track_ids_json())
    }

    /// Captures one layout string for persistence while retaining the native
    /// fingerprint and Rust-owned Aux identity used to reject stale snapshots.
    pub fn project_layout_snapshot_for_save(&self) -> (String, u64, String) {
        let aux_track_ids = self.aux_track_ids_json();
        let Some(engine) = self.engine.as_ref() else {
            return (String::new(), 0, aux_track_ids);
        };
        let raw = engine.get_project_layout_json();
        let native_fingerprint = crate::command_api::snapshot_generation(raw.as_bytes());
        let layout = Self::normalize_project_layout_json(raw, &aux_track_ids);
        (layout, native_fingerprint, aux_track_ids)
    }

    pub(crate) fn project_layout_snapshot_from_native_json(
        &self,
        raw: String,
    ) -> (String, String) {
        let aux_track_ids = self.aux_track_ids_json();
        let layout = Self::normalize_project_layout_json(raw, &aux_track_ids);
        (layout, aux_track_ids)
    }

    /// Normalizes and decodes an owned native layout without accessing Core or
    /// AudioEngine state. Save coordinators can run this expensive JSON work on
    /// a worker, then return the immutable document seed for Core enrichment.
    pub fn project_layout_document_seed(
        raw: String,
        aux_track_ids: &str,
        name: &str,
        bpm: f32,
        sample_rate: f64,
    ) -> anyhow::Result<(String, crate::project::ProjectDocument)> {
        let layout = Self::normalize_project_layout_json(raw, aux_track_ids);
        let document =
            crate::project::ProjectDocument::from_layout_json(name, bpm, sample_rate, &layout)?;
        Ok((layout, document))
    }

    fn normalize_project_layout_json(raw: String, aux_track_ids: &str) -> String {
        let Ok(mut layout) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return raw;
        };
        let Ok(aux_ids) = serde_json::from_str::<Vec<u32>>(aux_track_ids) else {
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

    /// Hashes the native track/region layout without copying its potentially
    /// large JSON representation across the CXX boundary. Rust-owned Aux and
    /// other control-plane data must still be fingerprinted separately.
    pub fn native_project_layout_fingerprint(&self) -> u64 {
        self.engine
            .as_ref()
            .map_or(0, |engine| engine.get_project_generation())
    }

    /// Cheap change token for integration polling. It advances when native
    /// layout mutators or Rust-owned Aux identity change; it is not a content
    /// hash and must not replace exact command-generation validation.
    pub fn project_layout_revision(&self) -> u64 {
        self.engine
            .as_ref()
            .map_or(0, |engine| engine.get_project_layout_revision())
    }

    /// Cheap project dirty token for UI autosave polling. Includes native
    /// layout and scheduled MIDI-note revisions without serializing the layout.
    pub fn project_state_revision(&self) -> u64 {
        let native = self.engine
            .as_ref()
            .map_or(0, |engine| engine.get_project_state_revision());
        let revisions = [
            native,
            self.scheduled_midi_notes.revision(),
            self.comping.revision(),
            self.aux_track_ids.revision(),
            self.midi_events.revision(),
            self.control_room.revision(),
            self.chord_track.revision(),
            self.openutau_vocals.revision(),
            self.track_stacks.revision(),
            self.markers.revision(),
            self.macro_mappings.revision(),
            self.midi_learn_mappings.revision(),
        ];
        revisions.into_iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, revision| {
            (hash ^ revision).wrapping_mul(0x100_0000_01b3)
        })
    }

    /// Fingerprints the normalized project layout exposed to external
    /// integrations. The native engine owns track and region state; Aux role
    /// identity is Rust-owned and must participate separately.
    pub fn project_layout_fingerprint(&self) -> u64 {
        let mut fingerprint = 0xcbf2_9ce4_8422_2325u64;
        let aux_track_ids = self.aux_track_ids_json();
        for byte in self
            .native_project_layout_fingerprint()
            .to_le_bytes()
            .into_iter()
            .chain(aux_track_ids.bytes())
        {
            fingerprint ^= u64::from(byte);
            fingerprint = fingerprint.wrapping_mul(0x100_0000_01b3);
        }
        fingerprint
    }

}
