struct HistoryLock {
    path: PathBuf,
    nonce: String,
}

impl HistoryLock {
    fn acquire(root: &Path) -> Result<Self> {
        let path = root.join("LOCK");
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) => {
                if reclaim_dead_lock(&path) {
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map_err(|retry| {
                            anyhow::anyhow!("history lock could not be reclaimed: {retry}")
                        })?
                } else {
                    bail!("history transaction is already in progress: {error}");
                }
            }
        };
        let nonce = Uuid::new_v4().to_string();
        let token = format!("pid={} nonce={}\n", std::process::id(), nonce);
        if let Err(error) = file
            .write_all(token.as_bytes())
            .and_then(|_| file.sync_all())
        {
            let _ = std::fs::remove_file(&path);
            return Err(error.into());
        }
        Ok(Self { path, nonce })
    }
}

impl Drop for HistoryLock {
    fn drop(&mut self) {
        let owns_lock = read_to_string(&self.path)
            .ok()
            .and_then(|contents| {
                contents
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("nonce=").map(str::to_owned))
            })
            .is_some_and(|nonce| nonce == self.nonce);
        if owns_lock {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn reclaim_dead_lock(path: &Path) -> bool {
    let Ok(contents) = read_to_string(path) else {
        return false;
    };
    let Some(pid) = contents
        .split_whitespace()
        .find_map(|field| field.strip_prefix("pid=")?.parse::<i32>().ok())
    else {
        return false;
    };
    #[cfg(unix)]
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(true);
    #[cfg(not(unix))]
    let alive = true;
    !alive && std::fs::remove_file(path).is_ok()
}
