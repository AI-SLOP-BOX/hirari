#[cfg(test)]
fn wav_metadata(bytes: &[u8]) -> (Option<u32>, Option<u16>, Option<u64>) {
    if bytes.len() < 12
        || (&bytes[0..4] != b"RIFF" && &bytes[0..4] != b"RF64")
        || &bytes[8..12] != b"WAVE"
    {
        return (None, None, None);
    }
    let mut offset = 12usize;
    let mut sample_rate = None;
    let mut channels = None;
    let mut block_align = None;
    let mut data_bytes = None;
    let mut rf64_data_bytes = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size32 = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
        let start = offset + 8;
        if start > bytes.len() {
            return (None, None, None);
        }
        if id == b"ds64" && size32 >= 16 && start + 16 <= bytes.len() {
            rf64_data_bytes = Some(u64::from_le_bytes(
                bytes[start + 8..start + 16].try_into().unwrap(),
            ));
        }
        let declared_size = if id == b"data" && size32 == u32::MAX {
            match rf64_data_bytes {
                Some(size) => size,
                None => return (None, None, None),
            }
        } else {
            u64::from(size32)
        };
        let payload_size = match usize::try_from(declared_size) {
            Ok(size) => size,
            Err(_) => return (None, None, None),
        };
        let _payload_end = match start.checked_add(payload_size) {
            Some(end) if end <= bytes.len() => end,
            _ => return (None, None, None),
        };
        if id == b"fmt " && declared_size >= 16 {
            channels = Some(u16::from_le_bytes(bytes[start + 2..start + 4].try_into().unwrap()));
            sample_rate = Some(u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap()));
            block_align = Some(u16::from_le_bytes(bytes[start + 12..start + 14].try_into().unwrap()));
        } else if id == b"data" {
            data_bytes = Some(declared_size);
        }
        let padded_size = match declared_size.checked_add(declared_size & 1) {
            Some(size) => size,
            None => return (None, None, None),
        };
        let padded_size = match usize::try_from(padded_size) {
            Ok(size) => size,
            Err(_) => return (None, None, None),
        };
        offset = match start.checked_add(padded_size) {
            Some(next) => next,
            None => return (None, None, None),
        };
        if offset > bytes.len() {
            return (None, None, None);
        }
        if data_bytes.is_some() && sample_rate.is_some() && channels.is_some() {
            break;
        }
    }
    let frames = match (data_bytes, block_align) {
        (Some(data), Some(align)) if align > 0 => Some(data / u64::from(align)),
        _ => None,
    };
    (sample_rate, channels, frames)
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::wav_metadata;

    #[test]
    fn application_path_uses_default_or_explicit_override() {
        assert_eq!(application_path_from(None), Path::new(DEFAULT_APP_PATH));
        assert_eq!(
            application_path_from(Some(std::ffi::OsString::from("/tmp/OpenUtau.app"))),
            Path::new("/tmp/OpenUtau.app")
        );
        assert_eq!(application_path_from(Some(std::ffi::OsString::new())), Path::new(DEFAULT_APP_PATH));
    }
    use std::io::Write;

    #[test]
    fn accepts_project_references_even_when_assets_are_not_local() {
        assert!(validate_source("voice/chorus.ustx").is_ok());
        assert!(validate_render("audio/chorus.wav").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_source_and_render_files() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "aura-openutau-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("voice.ustx");
        let render = root.join("voice.wav");
        let source_target = root.join("real.ustx");
        let render_target = root.join("real.wav");
        std::fs::write(&source_target, b"ustx_version: \"0.7\"\n").unwrap();
        std::fs::write(&render_target, b"RIFF").unwrap();

        symlink(&source_target, &source).unwrap();
        std::fs::copy(&render_target, &render).unwrap();
        let error = validate_import_files(source.to_str().unwrap(), render.to_str().unwrap()).unwrap_err();
        assert!(error.contains("source") && error.contains("symbolic link"));

        std::fs::remove_file(&source).unwrap();
        std::fs::remove_file(&render).unwrap();
        symlink(&render_target, &render).unwrap();
        let error = validate_import_files(source_target.to_str().unwrap(), render.to_str().unwrap()).unwrap_err();
        assert!(error.contains("rendered audio") && error.contains("symbolic link"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_non_openutau_extensions() {
        assert!(validate_source("voice/chorus.mid").is_err());
        assert!(validate_render("audio/chorus.mp3").is_err());
    }

    #[test]
    fn audit_records_content_identity_and_wav_shape() {
        let root = std::env::temp_dir().join(format!("aura-openutau-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("voice.ustx");
        let render = root.join("voice.wav");
        std::fs::write(&source, b"project-version: 0.1\n").unwrap();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&192_000u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&[0, 0, 0, 0]);
        std::fs::File::create(&render).unwrap().write_all(&wav).unwrap();
        let audit = audit_import_files(source.to_str().unwrap(), render.to_str().unwrap()).unwrap();
        assert_eq!(audit.rendered_audio_bytes, wav.len() as u64);
        assert_eq!(audit.rendered_sample_rate, Some(48_000));
        assert_eq!(audit.rendered_channels, Some(2));
        assert_eq!(audit.rendered_frames, Some(1));
        assert_eq!(audit.source_hash.len(), 64);
        assert_eq!(audit.rendered_audio_hash.len(), 64);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn audits_ustx_note_count_and_singer_identity() {
        let root = std::env::temp_dir().join(format!("aura-openutau-shape-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("voice.ustx");
        let render = root.join("voice.wav");
        std::fs::write(
            &source,
            "ustx_version: \"0.7\"\ntracks:\n- singer: KasaneTetoOfficial\nvoice_parts:\nnotes:\n  - position: 0\n  - position: 480\n",
        )
        .unwrap();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&44_100u32.to_le_bytes());
        wav.extend_from_slice(&88_200u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&2u32.to_le_bytes());
        wav.extend_from_slice(&[0, 0]);
        std::fs::write(&render, wav).unwrap();
        let audit = audit_import_files(source.to_str().unwrap(), render.to_str().unwrap()).unwrap();
        assert_eq!(audit.source_note_count, 2);
        assert_eq!(audit.source_singers, vec!["KasaneTetoOfficial"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_non_audio_openutau_render_payloads() {
        let root = std::env::temp_dir().join(format!("aura-openutau-invalid-render-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("voice.ustx");
        let render = root.join("voice.wav");
        std::fs::write(&source, "ustx_version: \"0.7\"\n").unwrap();
        std::fs::write(&render, b"not-a-wav").unwrap();
        let error = audit_import_files(source.to_str().unwrap(), render.to_str().unwrap()).unwrap_err();
        assert!(error.contains("valid non-empty RIFF/WAVE"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn accepts_rf64_and_unknown_odd_chunks_without_overreading() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RF64");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"ds64");
        bytes.extend_from_slice(&28u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&2u64.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"JUNK");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&44_100u32.to_le_bytes());
        bytes.extend_from_slice(&88_200u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0, 0]);
        assert_eq!(wav_metadata(&bytes), (Some(44_100), Some(1), Some(1)));
    }
}
