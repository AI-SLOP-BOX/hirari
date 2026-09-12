/// Add bounded, machine-actionable changes for array-backed project entities.
/// The section-level hashes remain the compatibility contract, while these
/// entries let CLI/LLM clients show which track, region, plugin, mapping, or
/// render target changed without diffing an entire project snapshot locally.
fn append_entity_changes(
    section: &str,
    before: &serde_json::Value,
    after: &serde_json::Value,
    changes: &mut Vec<SnapshotChange>,
) {
    let key = match section {
        "tracks" | "regions" => "id",
        "plugin_instances" => "instance_id",
        "midi_learn_mappings" => "mapping_id",
        "midi_notes" => "",
        "macro_mappings" => "mapping_id",
        "warp_markers" => "marker_id",
        "render_targets" => "target_id",
        "openutau_vocals" => "source_path",
        "freeze_artifacts" => "track_id",
        "audio_routes" | "feedback_routes" => "source_destination",
        _ => return,
    };
    let (Some(before_items), Some(after_items)) = (before.as_array(), after.as_array()) else {
        return;
    };

    let mut before_by_id = std::collections::BTreeMap::new();
    let mut after_by_id = std::collections::BTreeMap::new();
    for item in before_items {
        if let Some(id) = entity_id(item, key) {
            before_by_id.insert(id, item);
        }
    }
    for item in after_items {
        if let Some(id) = entity_id(item, key) {
            after_by_id.insert(id, item);
        }
    }

    let mut ids = std::collections::BTreeSet::new();
    ids.extend(before_by_id.keys().cloned());
    ids.extend(after_by_id.keys().cloned());
    for id in ids {
        let before_item = before_by_id.get(&id).copied();
        let after_item = after_by_id.get(&id).copied();
        let (operation, before_hash, after_hash) = match (before_item, after_item) {
            (None, Some(item)) => (
                "added",
                content_hash(&[]),
                content_hash(&serde_json::to_vec(item).unwrap_or_default()),
            ),
            (Some(item), None) => (
                "removed",
                content_hash(&serde_json::to_vec(item).unwrap_or_default()),
                content_hash(&[]),
            ),
            (Some(before_item), Some(after_item)) => {
                let before_bytes = serde_json::to_vec(before_item).unwrap_or_default();
                let after_bytes = serde_json::to_vec(after_item).unwrap_or_default();
                if before_bytes == after_bytes {
                    continue;
                }
                (
                    "changed",
                    content_hash(&before_bytes),
                    content_hash(&after_bytes),
                )
            }
            (None, None) => continue,
        };
        changes.push(SnapshotChange {
            section: section.to_owned(),
            before_hash,
            after_hash,
            entity_id: Some(id),
            operation: operation.to_owned(),
            fields: field_changes(before_item, after_item),
        });
    }
}

fn entity_id(item: &serde_json::Value, key: &str) -> Option<String> {
    if key == "source_destination" {
        let object = item.as_object()?;
        return Some(format!(
            "{}:{}",
            object.get("source_id")?.as_u64()?,
            object.get("destination_id")?.as_u64()?
        ));
    }
    if key.is_empty() {
        let object = item.as_object()?;
        let track = object.get("track_id")?.as_u64()?;
        let pitch = object.get("pitch")?.as_u64()?;
        let velocity = object.get("velocity")?.as_u64()?;
        let start = object.get("start_sample")?.as_u64()?;
        let length = object.get("length_samples")?.as_u64()?;
        return Some(format!("{track}:{pitch}:{velocity}:{start}:{length}"));
    }
    let value = item.get(key)?;
    match value {
        serde_json::Value::String(value) if !value.is_empty() => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn field_changes(
    before: Option<&serde_json::Value>,
    after: Option<&serde_json::Value>,
) -> Vec<FieldChange> {
    let mut names = std::collections::BTreeSet::new();
    for value in [before, after].into_iter().flatten() {
        if let Some(object) = value.as_object() {
            names.extend(object.keys().cloned());
        }
    }
    names
        .into_iter()
        .filter(|name| {
            !matches!(
                name.as_str(),
                "plugin_states"
                    | "plugin_state_hex"
                    | "sandbox_plugin_states"
                    | "sandbox_plugin_state_hex"
                    | "state_blob"
            )
        })
        .filter_map(|field| {
            let before_value = before.and_then(|value| value.get(&field)).cloned();
            let after_value = after.and_then(|value| value.get(&field)).cloned();
            (before_value != after_value).then_some(FieldChange {
                field,
                before: before_value,
                after: after_value,
            })
        })
        .take(32)
        .collect()
}

/// Hash an asset without loading the entire recording into memory. WAV
/// metadata is intentionally best-effort: a valid file may contain a large
/// unknown chunk before `fmt`/`data`, so the content hash remains authoritative
/// even when the bounded metadata probe cannot reach those chunks.
type AssetFingerprintData = (u64, String, Option<u32>, Option<u16>, Option<u64>);

fn fingerprint_asset_file(path: &Path) -> std::io::Result<AssetFingerprintData> {
    const HASH_CHUNK_BYTES: usize = 1024 * 1024;
    let mut file = File::open(path)?;
    let byte_count = file.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut probe = Vec::with_capacity(HASH_CHUNK_BYTES.min(byte_count as usize));
    let mut buffer = vec![0u8; HASH_CHUNK_BYTES];
    loop {
        let read_bytes = file.read(&mut buffer)?;
        if read_bytes == 0 {
            break;
        }
        hasher.update(&buffer[..read_bytes]);
        if probe.len() < HASH_CHUNK_BYTES {
            let remaining = HASH_CHUNK_BYTES - probe.len();
            probe.extend_from_slice(&buffer[..read_bytes.min(remaining)]);
        }
    }
    let digest = hasher.finalize();
    let hash = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let (sample_rate, channels, frame_count) = wav_metadata(&probe);
    Ok((byte_count, hash, sample_rate, channels, frame_count))
}

fn commit_id_material(commit: &ProjectCommit) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        commit.snapshot_hash,
        commit.parent.as_deref().unwrap_or(""),
        commit.branch,
        commit.message,
        commit.created_unix_seconds,
        commit.project_id,
        commit.project_format_version,
        commit.asset_manifest_hash,
        commit.plugin_manifest_hash,
        commit.platform,
        commit.render_artifact_hash,
        commit.render_artifact_bytes,
    )
}

fn validate_ref_name(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name.len() > 128
        || name != name.trim()
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.contains("..")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        || name.starts_with('.')
    {
        bail!("invalid history ref name");
    }
    Ok(())
}

fn validate_commit_id(id: &str) -> Result<()> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid commit id");
    }
    Ok(())
}

fn wav_metadata(bytes: &[u8]) -> (Option<u32>, Option<u16>, Option<u64>) {
    if bytes.len() < 12
        || (&bytes[0..4] != b"RIFF" && &bytes[0..4] != b"RF64")
        || &bytes[8..12] != b"WAVE"
    {
        return (None, None, None);
    }
    let mut cursor = 12usize;
    let mut sample_rate = None;
    let mut channels = None;
    let mut block_align = None;
    let mut data_bytes = None;
    while cursor.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let id = &bytes[cursor..cursor + 4];
        let size = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        cursor += 8;
        let end = match cursor.checked_add(size) {
            Some(end) if end <= bytes.len() => end,
            _ => break,
        };
        if id == b"fmt " && size >= 16 {
            channels = Some(u16::from_le_bytes(
                bytes[cursor + 2..cursor + 4].try_into().unwrap(),
            ));
            sample_rate = Some(u32::from_le_bytes(
                bytes[cursor + 4..cursor + 8].try_into().unwrap(),
            ));
            block_align = Some(u16::from_le_bytes(
                bytes[cursor + 12..cursor + 14].try_into().unwrap(),
            ));
        } else if id == b"data" {
            data_bytes = Some(size as u64);
        }
        cursor = end + (size & 1);
    }
    let frames = data_bytes
        .zip(block_align)
        .filter(|(_, align)| *align > 0)
        .map(|(data, align)| data / u64::from(align));
    (sample_rate, channels, frames)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("entry"),
        std::process::id(),
        unique_nonce()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    if let Err(error) = rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn remove_published_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn unique_nonce() -> u128 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed) as u128;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    nanos ^ ((std::process::id() as u128) << 64) ^ sequence
}
