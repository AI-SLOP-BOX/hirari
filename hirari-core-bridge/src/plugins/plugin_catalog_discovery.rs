fn roots() -> Vec<PathBuf> {
    let mut result = vec![
        PathBuf::from("/Library/Audio/Plug-Ins/CLAP"),
        PathBuf::from("/Library/Audio/Plug-Ins/VST3"),
        PathBuf::from("/Library/Audio/Plug-Ins/Components"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        result.extend([
            home.join("Library/Audio/Plug-Ins/CLAP"),
            home.join("Library/Audio/Plug-Ins/VST3"),
            home.join("Library/Audio/Plug-Ins/Components"),
        ]);
    }
    // Allow installations outside the platform defaults (portable plugin
    // folders, per-project tools, CI fixtures, and user-managed bundles)
    // without recompiling Hirari. `split_paths` uses the native path-list
    // separator and therefore works on macOS, Linux, and Windows.
    if let Some(extra) = std::env::var_os("HIRARI_PLUGIN_PATHS") {
        result.extend(std::env::split_paths(&extra));
    }
    result
}

fn format_for(path: &std::path::Path) -> Option<&'static str> {
    match crate::plugin_admission::classify_format(path)? {
        "CLAP" => Some("clap"),
        "VST3" => Some("vst3"),
        "AU" => Some("au"),
        _ => None,
    }
}

fn canonical(path: PathBuf) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

fn collect_candidates(root: &std::path::Path, candidates: &mut Vec<PathBuf>) {
    if !crate::plugin_admission::safe_candidate(root, "") {
        return;
    }
    let Ok(metadata) = std::fs::symlink_metadata(root) else {
        return;
    };
    if !metadata.is_dir() {
        if format_for(root).is_some() {
            candidates.push(root.to_path_buf());
        }
        return;
    }
    // A plugin bundle is itself a directory. Do not descend into it: its
    // implementation files can carry misleading extensions and scanning
    // them would create duplicate catalog entries. Only recurse through
    // ordinary user-created folders.
    if format_for(root).is_some() {
        candidates.push(root.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        collect_candidates(&entry.path(), candidates);
    }
}

fn binary_identity(path: &std::path::Path) -> (String, u64, u64) {
    let mut files = Vec::new();
    collect_files(path, path, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    let mut bytes_total = 0u64;
    let mut newest = 0u64;
    for (relative, file) in files {
        let Ok(mut input) = std::fs::File::open(&file) else {
            return (String::new(), 0, newest);
        };
        let Ok(metadata) = input.metadata() else {
            return (String::new(), 0, newest);
        };
        let file_bytes = metadata.len();
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_secs());

        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(file_bytes.to_le_bytes());
        let mut read_bytes = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = match std::io::Read::read(&mut input, &mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(_) => return (String::new(), 0, newest),
            };
            read_bytes = read_bytes.saturating_add(count as u64);
            hasher.update(&buffer[..count]);
        }
        // The size is part of the hash framing. If a plugin is replaced or
        // modified while scanning, discard the partial identity rather than
        // recording a fingerprint for a byte sequence with the wrong length.
        if read_bytes != file_bytes {
            return (String::new(), 0, newest);
        }
        bytes_total = bytes_total.saturating_add(read_bytes);
        newest = newest.max(modified);
    }
    let hash = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    (hash, bytes_total, newest)
}

/// Control-plane fingerprint used when a plugin instance is serialized into
/// a project. Missing plugins intentionally return `None`; an absent binary
/// must be represented as unavailable, not as the hash of an empty file.
pub fn binary_hash_for_path(path: &str) -> Option<String> {
    let path = std::path::Path::new(path);
    if !path.exists() {
        return None;
    }
    let (hash, bytes, _) = binary_identity(path);
    (bytes > 0).then_some(hash)
}

fn collect_files(
    root: &std::path::Path,
    path: &std::path::Path,
    files: &mut Vec<(String, PathBuf)>,
) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_file() {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        files.push((relative, path.to_path_buf()));
        return;
    }
    if !metadata.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        collect_files(root, &entry.path(), files);
    }
}

#[derive(Clone, Copy, Serialize)]
struct WorkerCapabilities {
    clap: bool,
    au: bool,
    vst3: bool,
}

fn worker_capabilities() -> WorkerCapabilities {
    let mut candidates = Vec::new();
    if let Some(configured) = std::env::var_os("HIRARI_PLUGIN_HOST_BIN") {
        candidates.push(PathBuf::from(configured));
    }
    candidates.extend([
        PathBuf::from("build-tools/hirari-plugin-host-worker"),
        PathBuf::from("/usr/local/bin/hirari-plugin-host-worker"),
    ]);
    // Match the native host's packaged-app lookup. Without this, the UI can
    // report every installed plugin as unavailable even though the worker is
    // correctly embedded beside the shipped application executable.
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join("hirari-plugin-host-worker"));
            candidates.push(parent.join("Contents/MacOS/hirari-plugin-host-worker"));
            if let Some(contents) = parent.parent() {
                candidates.push(contents.join("MacOS/hirari-plugin-host-worker"));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for candidate in candidates {
        if !seen.insert(candidate.clone()) {
            continue;
        }
        let Ok(output) = std::process::Command::new(candidate)
            .arg("--capabilities")
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
            return WorkerCapabilities {
                clap: value
                    .get("clap")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                au: value
                    .get("au")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                vst3: value
                    .get("vst3")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            };
        }
    }
    WorkerCapabilities {
        clap: false,
        au: false,
        vst3: false,
    }
}

fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn scan() -> Vec<InstalledPlugin> {
    // Keep one entry per concrete binary/format.  A previous implementation
    // keyed by the display name, which silently discarded e.g. Vital's VST3
    // when its CLAP was also installed.  The browser needs to show every
    // available format so future plugins require no UI special case.
    let capabilities = worker_capabilities();
    let mut found = BTreeMap::<String, InstalledPlugin>::new();
    for root in roots() {
        let mut candidates = Vec::new();
        collect_candidates(&root, &mut candidates);
        for path in candidates {
            let Some(format) = format_for(&path) else {
                continue;
            };
            let Some(path) = canonical(path) else {
                continue;
            };
            let Some(file_name) = path.file_stem().and_then(|v| v.to_str()) else {
                continue;
            };
            let (binary_hash, binary_bytes, modified_unix_seconds) = binary_identity(&path);
            // A bundle that cannot be read is not an installed/admissible
            // plugin.  Publishing it with the hash of an empty traversal
            // makes a permission error look like a valid cache entry and can
            // later fail much deeper in the sandbox admission path.
            if binary_bytes == 0 || binary_hash.is_empty() {
                continue;
            }
            let name = file_name.to_owned();
            let key = normalized(&name);
            if key.is_empty() {
                continue;
            }
            // Keep every concrete format visible.  Whether it can be
            // instantiated is reported by the worker capability probe below.
            let path_key = path.to_string_lossy().to_ascii_lowercase();
            let id = format!("{key}.{format}.{}", content_fingerprint(&path_key));
            let quarantined = is_quarantined_hash(&binary_hash);
            let candidate = InstalledPlugin {
                id: id.clone(),
                name,
                format: format.to_owned(),
                path: path.to_string_lossy().into_owned(),
                installed: true,
                sandboxed: match format {
                    "clap" => capabilities.clap,
                    "vst3" => capabilities.vst3,
                    _ => capabilities.au,
                },
                capability: if quarantined {
                    "quarantined".to_owned()
                } else {
                    match format {
                        "clap" if capabilities.clap => "sandbox-ready".to_owned(),
                        "vst3" if capabilities.vst3 => "vst3-sandbox-ready".to_owned(),
                        "vst3" => "vst3-sdk-required".to_owned(),
                        "au" if capabilities.au => "au-sandbox-ready".to_owned(),
                        "au" => "au-worker-unavailable".to_owned(),
                        _ => "worker-unavailable".to_owned(),
                    }
                },
                binary_hash,
                binary_bytes,
                modified_unix_seconds,
                quarantined,
                tags: vec![format.to_owned()],
                favorite: false,
            };
            found.entry(id).or_insert(candidate);
        }
    }
    found.into_values().collect()
}

fn content_fingerprint(value: &str) -> String {
    // This is only a stable catalog key, not an integrity hash.  Binary
    // identity is supplied by the native admission layer when instantiated.
    // Include the full canonical path so same-named plugins remain selectable.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn resolve(alias: &str) -> Option<InstalledPlugin> {
    let (name_alias, requested_format) = alias
        .split_once('@')
        .or_else(|| alias.split_once(':'))
        .map_or((alias, None), |(name, format)| {
            (name, Some(format.to_ascii_lowercase()))
        });
    let requested = normalized(name_alias);
    let mut plugins = vec![
        InstalledPlugin {
            id: "hirari.internal.limiter".into(),
            name: "Hirari Limiter".into(),
            format: "internal".into(),
            path: "Hirari/Limiter".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime dynamics".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["dynamics".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "hirari.internal.compressor".into(),
            name: "Hirari Compressor".into(),
            format: "internal".into(),
            path: "Hirari/Compressor".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime dynamics · assistant-ready".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["dynamics".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "hirari.internal.gate".into(),
            name: "Hirari Gate".into(),
            format: "internal".into(),
            path: "Hirari/Gate".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime noise gate".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["dynamics".into(), "gate".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "hirari.internal.saturation".into(),
            name: "Hirari Saturation".into(),
            format: "internal".into(),
            path: "Hirari/Saturation".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime tube saturation".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["color".into(), "saturation".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "hirari.internal.transient".into(),
            name: "Hirari Transient Shaper".into(),
            format: "internal".into(),
            path: "Hirari/Transient".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime transient shaping".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["dynamics".into(), "transient".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "hirari.internal.deesser".into(),
            name: "Hirari De-Esser".into(),
            format: "internal".into(),
            path: "Hirari/DeEsser".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime sibilance reduction".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["dynamics".into(), "de-esser".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "hirari.internal.delay".into(),
            name: "Hirari Delay".into(),
            format: "internal".into(),
            path: "Hirari/Delay".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime tempo-synced delay".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["delay".into(), "time-based".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "hirari.internal.reverb".into(),
            name: "Hirari Reverb".into(),
            format: "internal".into(),
            path: "Hirari/Reverb".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime algorithmic reverb".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["reverb".into(), "time-based".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "hirari.internal.dynamiceq".into(),
            name: "Hirari Dynamic EQ".into(),
            format: "internal".into(),
            path: "Hirari/DynamicEQ".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime dynamic equalization".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["eq".into(), "dynamic".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "hirari.internal.midside".into(),
            name: "Hirari Mid/Side".into(),
            format: "internal".into(),
            path: "Hirari/MidSide".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime mid-side processing".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["mid-side".into(), "stereo".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "hirari.internal.width".into(),
            name: "Hirari Stereo Width".into(),
            format: "internal".into(),
            path: "Hirari/Width".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime stereo width".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["stereo".into(), "width".into(), "internal".into()],
            favorite: false,
        },
    ];
    plugins.extend(scan());

    // An explicit catalog id is an unambiguous request and must win even
    // when the same plugin is installed in several formats.  Name-based
    // lookup is intentionally deterministic instead of depending on the
    // BTreeMap/hash suffix ordering used by discovery.
    if let Some(plugin) = plugins
        .iter()
        .find(|plugin| plugin.id == alias || normalized(&plugin.id) == normalized(alias))
    {
        return Some(plugin.clone());
    }
    const FORMAT_PRIORITY: [&str; 3] = ["clap", "vst3", "au"];
    plugins.sort_by_key(|plugin| {
        let format_rank = FORMAT_PRIORITY
            .iter()
            .position(|format| *format == plugin.format)
            .unwrap_or(FORMAT_PRIORITY.len());
        (format_rank, plugin.path.to_ascii_lowercase())
    });
    plugins.into_iter().find(|plugin| {
        normalized(&plugin.name) == requested
            && requested_format
                .as_deref()
                .is_none_or(|format| format == plugin.format)
    })
}

/// Return whether a concrete path belongs to a plugin discovered by the
/// installed-plugin scanner.  This is used by the command permission layer:
/// project-write clients may select system/user plugin bundles, but may not
/// turn an arbitrary executable path into a plugin load request.
pub fn is_admitted_path(path: &str) -> bool {
    let Ok(requested) = std::fs::canonicalize(path) else {
        return false;
    };
    scan().into_iter().any(|plugin| {
        plugin.installed
            && plugin.sandboxed
            && !plugin.quarantined
            && std::fs::canonicalize(&plugin.path).is_ok_and(|candidate| candidate == requested)
    })
}
