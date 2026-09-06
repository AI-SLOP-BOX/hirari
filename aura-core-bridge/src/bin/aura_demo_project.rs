//! Build a small, portable Aura demo project from rendered audio assets.
//! This intentionally uses the same AuraCore mutation boundary as the UI.

use aura_core_bridge::AuraCore;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let output = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("output project path is required"))?;
    let synth = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("synth WAV path is required"))?;
    let vocal = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("vocal WAV path is required"))?;
    let source = args.next();
    if args.next().is_some() {
        anyhow::bail!(
            "usage: aura_demo_project <output.aura> <synth.wav> <vocal.wav> [vocal.ustx]"
        );
    }
    let core = AuraCore::new()?;
    core.new_project();
    let synth_track = core.add_track(0);
    let vocal_track = core.add_track(0);
    anyhow::ensure!(
        synth_track != 0 && vocal_track != 0,
        "failed to create audio tracks"
    );
    anyhow::ensure!(
        core.set_track_name(synth_track, "NEON SYNTH · ORIGINAL"),
        "failed to name synth track"
    );
    anyhow::ensure!(
        core.set_track_name(vocal_track, "重音テト · UTAU VOCAL"),
        "failed to name vocal track"
    );
    anyhow::ensure!(
        core.add_region(synth_track, &synth, 0.0),
        "failed to add synth region"
    );
    anyhow::ensure!(
        core.add_region(vocal_track, &vocal, 0.0),
        "failed to add vocal region"
    );
    let mut imported_midi_notes = 0usize;
    if let Some(source) = source.as_ref() {
        core.register_openutau_vocal(source, &vocal)
            .map_err(|error| anyhow::anyhow!("failed to register OpenUtau vocal: {error}"))?;
        let note_document: serde_json::Value = serde_json::from_str(
            &core.openutau_midi_notes_at_bpm_json(source, vocal_track, 44_100, 480, 172.0),
        )?;
        for note in note_document
            .get("notes")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(pitch) = note.get("pitch").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            let Some(start_sample) = note.get("start_sample").and_then(serde_json::Value::as_u64)
            else {
                continue;
            };
            let Some(length_samples) = note
                .get("length_samples")
                .and_then(serde_json::Value::as_u64)
            else {
                continue;
            };
            let lyric = note
                .get("lyric")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            anyhow::ensure!(
                core.set_midi_note_lyric(
                    vocal_track,
                    pitch as u8,
                    100,
                    start_sample,
                    length_samples,
                    lyric,
                ),
                "failed to add OpenUtau MIDI note"
            );
            imported_midi_notes += 1;
        }
    }
    anyhow::ensure!(
        core.save_project(&output),
        "failed to save native Aura project"
    );
    println!(
        "{{\"ok\":true,\"project\":{:?},\"tracks\":2,\"regions\":2,\"openutau\":{},\"midi_notes\":{}}}",
        output,
        source.is_some(),
        imported_midi_notes
    );
    Ok(())
}
