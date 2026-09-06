#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub transaction_id: Option<String>,
    pub state: LedgerState,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub prepared_unix_seconds: u64,
}

/// Read-only audit projection used by CLI/AI clients. It intentionally omits
/// internal ledger details while retaining enough information to inspect a
/// request, identify its transaction, and replay a committed outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEntry {
    pub request_id: String,
    pub transaction_id: Option<String>,
    pub state: LedgerState,
    pub prepared_unix_seconds: u64,
    pub replayable: bool,
    pub result: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LedgerBegin {
    Started,
    Replayed(serde_json::Value),
}

/// Outcome of an idempotent mutation submission.  A replay is successful and
/// returns the original durable result; callers should not execute the native
/// side effect again.
#[derive(Debug, Clone, PartialEq)]
pub enum LedgerOutcome {
    Applied(serde_json::Value),
    Replayed(serde_json::Value),
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RequestLedger {
    entries: std::collections::BTreeMap<String, LedgerEntry>,
}

struct LedgerLock {
    path: PathBuf,
    nonce: String,
}

impl LedgerLock {
    fn acquire(ledger_path: &Path) -> Result<Self, BridgeError> {
        let path = ledger_path.with_extension("lock");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| BridgeError::new("ledger_directory_failed", error.to_string()))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .or_else(|error| {
                if reclaim_dead_ledger_lock(&path) {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                } else {
                    Err(error)
                }
            })
            .map_err(|error| BridgeError::new("ledger_busy", error.to_string()))?;
        let nonce = Uuid::new_v4().to_string();
        let owner = format!("pid={} nonce={nonce}\n", std::process::id());
        if let Err(error) = file
            .write_all(owner.as_bytes())
            .and_then(|_| file.sync_all())
        {
            let _ = std::fs::remove_file(&path);
            return Err(BridgeError::new("ledger_lock_failed", error.to_string()));
        }
        Ok(Self { path, nonce })
    }
}

fn reclaim_dead_ledger_lock(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
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

impl Drop for LedgerLock {
    fn drop(&mut self) {
        let owns_lock = std::fs::read_to_string(&self.path)
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

/// Holds the project-wide apply lock for the entire native transaction. The
/// ledger lock is intentionally short-lived so it can protect individual
/// durable updates; it must not be reused as the mutation lock because it is
/// released between `begin` and `complete`.
pub struct CommandTransactionLock {
    path: PathBuf,
    nonce: String,
}

impl CommandTransactionLock {
    pub fn acquire(ledger_path: impl AsRef<Path>) -> Result<Self, BridgeError> {
        let path = ledger_path.as_ref().with_extension("transaction.lock");
        // The transaction lock is acquired before RequestLedger::with_locked
        // gets a chance to create the ledger directory.  New projects
        // therefore need this directory creation here as well; otherwise the
        // first CLI mutation fails with a misleading transaction_busy/ENOENT.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| BridgeError::new("ledger_directory_failed", error.to_string()))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .or_else(|error| {
                if reclaim_dead_ledger_lock(&path) {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                } else {
                    Err(error)
                }
            })
            .map_err(|error| BridgeError::new("transaction_busy", error.to_string()))?;
        let nonce = Uuid::new_v4().to_string();
        let owner = format!("pid={} nonce={nonce}\n", std::process::id());
        if let Err(error) = file
            .write_all(owner.as_bytes())
            .and_then(|_| file.sync_all())
        {
            let _ = std::fs::remove_file(&path);
            return Err(BridgeError::new(
                "transaction_lock_failed",
                error.to_string(),
            ));
        }
        Ok(Self { path, nonce })
    }
}

impl Drop for CommandTransactionLock {
    fn drop(&mut self) {
        let owns_lock = std::fs::read_to_string(&self.path)
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

impl RequestLedger {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BridgeError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = read(path)
            .map_err(|error| BridgeError::new("ledger_read_failed", error.to_string()))?;
        if let Ok(ledger) = serde_json::from_slice(&bytes) {
            return Ok(ledger);
        }
        // Migrate the original result-only ledger format. Existing successful
        // entries are safe to replay and are treated as committed records.
        let legacy: std::collections::BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&bytes)
                .map_err(|error| BridgeError::new("ledger_corrupt", error.to_string()))?;
        Ok(Self {
            entries: legacy
                .into_iter()
                .map(|(key, result)| {
                    (
                        key,
                        LedgerEntry {
                            transaction_id: None,
                            state: LedgerState::Committed,
                            result: Some(result),
                            prepared_unix_seconds: 0,
                        },
                    )
                })
                .collect(),
        })
    }

    pub fn replay(&self, request_id: &str) -> Option<serde_json::Value> {
        self.entries
            .get(request_id)
            .and_then(|entry| match entry.state {
                LedgerState::Committed | LedgerState::Failed => entry.result.clone(),
                LedgerState::Prepared | LedgerState::Applying => None,
            })
    }

    /// Return a deterministic, read-only audit log for external automation.
    /// Results are included because a lost response must be recoverable
    /// without reapplying the mutation.
    pub fn audit_log(&self) -> Vec<AuditEntry> {
        self.entries
            .iter()
            .map(|(request_id, entry)| AuditEntry {
                request_id: request_id.clone(),
                transaction_id: entry.transaction_id.clone(),
                state: entry.state.clone(),
                prepared_unix_seconds: entry.prepared_unix_seconds,
                replayable: matches!(entry.state, LedgerState::Committed | LedgerState::Failed),
                result: entry.result.clone(),
            })
            .collect()
    }

    pub fn record(
        &mut self,
        request_id: &str,
        result: serde_json::Value,
        path: impl AsRef<Path>,
    ) -> Result<(), BridgeError> {
        self.record_once(request_id, None, result, path)
    }

    pub fn record_once(
        &mut self,
        request_id: &str,
        transaction_id: Option<&str>,
        result: serde_json::Value,
        path: impl AsRef<Path>,
    ) -> Result<(), BridgeError> {
        match self.begin(request_id, transaction_id, &path)? {
            LedgerBegin::Started => self.complete(request_id, result, &path),
            LedgerBegin::Replayed(_) => Err(BridgeError::new(
                "duplicate_request",
                "request or transaction has already been executed",
            )),
        }
    }

    /// Idempotent mutation helper for CLI/LLM clients.  Unlike the legacy
    /// `record_once`, a duplicate request is not surfaced as an error: the
    /// previously committed result is returned and the caller can safely
    /// present it as a replay.  This is the stable retry contract for clients
    /// that may lose the response after the side effect commits.
    pub fn record_or_replay(
        &mut self,
        request_id: &str,
        transaction_id: Option<&str>,
        result: serde_json::Value,
        path: impl AsRef<Path>,
    ) -> Result<LedgerOutcome, BridgeError> {
        match self.begin(request_id, transaction_id, &path)? {
            LedgerBegin::Started => {
                self.complete(request_id, result.clone(), &path)?;
                Ok(LedgerOutcome::Applied(result))
            }
            LedgerBegin::Replayed(previous) => Ok(LedgerOutcome::Replayed(previous)),
        }
    }

    /// Durably mark a prepared request as applying immediately before the
    /// native side effect.  Recovery can now distinguish a request that was
    /// only admitted from one that may have reached the external boundary.
    pub fn mark_applying(
        &mut self,
        request_id: &str,
        path: impl AsRef<Path>,
    ) -> Result<(), BridgeError> {
        let path = path.as_ref();
        self.with_locked(path, |ledger| {
            let Some(entry) = ledger.entries.get(request_id).cloned() else {
                return Err(BridgeError::new(
                    "ledger_missing",
                    "request was not prepared",
                ));
            };
            if !matches!(entry.state, LedgerState::Prepared) {
                return Err(BridgeError::new(
                    "ledger_not_prepared",
                    "request is not in prepared state",
                ));
            }
            let updated = LedgerEntry {
                state: LedgerState::Applying,
                ..entry.clone()
            };
            ledger
                .entries
                .insert(request_id.to_owned(), updated.clone());
            if let Some(transaction_id) = entry.transaction_id {
                ledger
                    .entries
                    .insert(format!("transaction:{transaction_id}"), updated);
            }
            Ok(())
        })
    }

    /// Persist the in-flight marker before any native side effect occurs.
    pub fn begin(
        &mut self,
        request_id: &str,
        transaction_id: Option<&str>,
        path: impl AsRef<Path>,
    ) -> Result<LedgerBegin, BridgeError> {
        let path = path.as_ref();
        self.with_locked(path, |ledger| {
            validate_ledger_ids(request_id, transaction_id)?;
            let transaction_key = transaction_id.map(|value| format!("transaction:{value}"));
            let existing = ledger.entries.get(request_id).or_else(|| {
                transaction_key
                    .as_deref()
                    .and_then(|key| ledger.entries.get(key))
            });
            if let Some(entry) = existing {
                return match entry.state {
                    LedgerState::Committed | LedgerState::Failed => Ok(LedgerBegin::Replayed(
                        entry
                            .result
                            .clone()
                            .unwrap_or_else(|| serde_json::json!({})),
                    )),
                    LedgerState::Prepared | LedgerState::Applying => Err(BridgeError::new(
                        "ledger_in_flight",
                        "request is already applying; reconciliation is required before retry",
                    )),
                };
            }
            let entry = LedgerEntry {
                transaction_id: transaction_id.map(str::to_owned),
                state: LedgerState::Prepared,
                result: None,
                prepared_unix_seconds: unix_seconds(),
            };
            ledger.entries.insert(request_id.to_owned(), entry.clone());
            if let Some(key) = transaction_key {
                ledger.entries.insert(key, entry);
            }
            Ok(LedgerBegin::Started)
        })
    }

    /// Publish the result of a mutation that was previously marked prepared.
    pub fn complete(
        &mut self,
        request_id: &str,
        result: serde_json::Value,
        path: impl AsRef<Path>,
    ) -> Result<(), BridgeError> {
        let path = path.as_ref();
        self.with_locked(path, |ledger| {
            let Some(entry) = ledger.entries.get(request_id).cloned() else {
                return Err(BridgeError::new(
                    "ledger_missing",
                    "request was not prepared",
                ));
            };
            if !matches!(entry.state, LedgerState::Prepared | LedgerState::Applying) {
                return Err(BridgeError::new(
                    "ledger_not_applying",
                    "request is not in flight",
                ));
            }
            let updated = LedgerEntry {
                state: LedgerState::Committed,
                result: Some(result),
                ..entry.clone()
            };
            ledger
                .entries
                .insert(request_id.to_owned(), updated.clone());
            if let Some(transaction_id) = entry.transaction_id {
                ledger
                    .entries
                    .insert(format!("transaction:{transaction_id}"), updated);
            }
            Ok(())
        })
    }

    /// Record a terminal failure so automatic retries cannot repeat a
    /// mutation whose rollback status is unknown.
    pub fn fail(
        &mut self,
        request_id: &str,
        result: serde_json::Value,
        path: impl AsRef<Path>,
    ) -> Result<(), BridgeError> {
        let path = path.as_ref();
        self.with_locked(path, |ledger| {
            let Some(entry) = ledger.entries.get(request_id).cloned() else {
                return Err(BridgeError::new(
                    "ledger_missing",
                    "request was not prepared",
                ));
            };
            let updated = LedgerEntry {
                state: LedgerState::Failed,
                result: Some(result),
                ..entry.clone()
            };
            ledger
                .entries
                .insert(request_id.to_owned(), updated.clone());
            if let Some(transaction_id) = entry.transaction_id {
                ledger
                    .entries
                    .insert(format!("transaction:{transaction_id}"), updated);
            }
            Ok(())
        })
    }

    fn with_locked<T>(
        &mut self,
        path: &Path,
        operation: impl FnOnce(&mut Self) -> Result<T, BridgeError>,
    ) -> Result<T, BridgeError> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)
                .map_err(|error| BridgeError::new("ledger_directory_failed", error.to_string()))?;
        }
        let _lock = LedgerLock::acquire(path)?;
        if path.exists() {
            *self = Self::open(path)?;
        }
        let result = operation(self)?;
        self.persist(path)?;
        Ok(result)
    }

    fn persist(&self, path: &Path) -> Result<(), BridgeError> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| BridgeError::new("ledger_encode_failed", error.to_string()))?;
        let temp = path.with_extension(format!(
            "tmp-{}-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
            crate::project_history::content_hash(&bytes)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| BridgeError::new("ledger_write_failed", error.to_string()))?;
        if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            let _ = std::fs::remove_file(&temp);
            return Err(BridgeError::new("ledger_write_failed", error.to_string()));
        }
        if let Err(error) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(BridgeError::new("ledger_publish_failed", error.to_string()));
        }
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    BridgeError::new("ledger_directory_sync_failed", error.to_string())
                })?;
        }
        Ok(())
    }
}


