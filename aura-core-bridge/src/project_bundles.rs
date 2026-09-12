impl ProjectDocument {
    pub fn export_track_bundle(&self, track_id: u32) -> Result<Vec<u8>> {
        if track_id == 0 { bail!("track id is invalid"); }
        let track = self.tracks.iter().find(|track| track.id == track_id).cloned().ok_or_else(|| anyhow::anyhow!("track not found"))?;
        let regions = self.regions.iter().filter(|region| region.track_id == track_id).cloned().collect::<Vec<_>>();
        let notes = self.midi_notes.iter().filter(|note| note.track_id == track_id).cloned().collect::<Vec<_>>();
        let payload = serde_json::json!({"format":"aura-track-bundle-v1","track":track,"regions":regions,"midi_notes":notes});
        let payload_bytes = serde_json::to_vec(&payload)?;
        let checksum = format!("{:x}", Sha256::digest(&payload_bytes));
        let bundle = serde_json::json!({"format":"aura-track-bundle-v1","track":payload["track"],"regions":payload["regions"],"midi_notes":payload["midi_notes"],"sha256":checksum});
        Ok(serde_json::to_vec(&bundle)?)
    }

    pub fn import_track_bundle(&mut self, bundle_data: &[u8], new_track_id: u32) -> Result<()> {
        if new_track_id == 0 { bail!("track id is invalid"); }
        let value: serde_json::Value = serde_json::from_slice(bundle_data).context("track bundle JSON is invalid")?;
        if value.get("format").and_then(|v| v.as_str()) != Some("aura-track-bundle-v1") { bail!("unsupported track bundle format"); }
        if let Some(expected) = value.get("sha256").and_then(|v| v.as_str()) {
            let payload = serde_json::json!({"format":"aura-track-bundle-v1","track":value.get("track"),"regions":value.get("regions").cloned().unwrap_or_else(|| serde_json::json!([])),"midi_notes":value.get("midi_notes").cloned().unwrap_or_else(|| serde_json::json!([]))});
            let actual = format!("{:x}", Sha256::digest(serde_json::to_vec(&payload)?));
            if expected != actual { bail!("track bundle checksum mismatch"); }
        }
        let mut track: ProjectTrack = serde_json::from_value(value.get("track").cloned().ok_or_else(|| anyhow::anyhow!("track bundle has no track"))?)?;
        if self.tracks.iter().any(|candidate| candidate.id == new_track_id) { bail!("destination track id already exists"); }
        track.id = new_track_id;
        let mut regions: Vec<ProjectRegion> = serde_json::from_value(value.get("regions").cloned().unwrap_or_else(|| serde_json::json!([])))?;
        let mut notes: Vec<MidiNoteContract> = serde_json::from_value(value.get("midi_notes").cloned().unwrap_or_else(|| serde_json::json!([])))?;
        for region in &mut regions { region.track_id = new_track_id; }
        for note in &mut notes { note.track_id = new_track_id; }
        let mut candidate = self.clone();
        candidate.tracks.push(track); candidate.regions.extend(regions); candidate.midi_notes.extend(notes);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
    /// Returns a portable, deterministic identity for the project and its
    /// external audio references.  Paths are retained as authored so the
    /// manifest describes the project contract without leaking host paths.
    pub fn reproducibility_manifest(&self) -> Result<serde_json::Value> {
        let snapshot = serde_json::to_vec(self)?;
        let snapshot_hash = format!("{:x}", Sha256::digest(&snapshot));
        let assets = self.regions.iter().map(|region| {
            serde_json::json!({
                "region_id": region.id,
                "track_id": region.track_id,
                "path": region.path,
                "source_offset": region.source_offset,
                "length": region.length,
            })
        }).collect::<Vec<_>>();
        let asset_bytes = serde_json::to_vec(&assets)?;
        Ok(serde_json::json!({
            "manifest_version": 1,
            "project_id": self.project_id,
            "schema_version": self.schema_version,
            "contract_version": self.contract_version,
            "sample_rate": self.sample_rate,
            "snapshot_sha256": snapshot_hash,
            "asset_reference_sha256": format!("{:x}", Sha256::digest(&asset_bytes)),
            "asset_references": assets,
            "track_count": self.tracks.len(),
            "region_count": self.regions.len(),
            "plugin_count": self.plugin_instances.len(),
        }))
    }
}
