use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};

struct RecoverySummaryJob {
    project_path: String,
}

struct RecoverySummaryCompletion {
    project_path: String,
    summary: RecoverySummary,
}

/// Keeps recovery-generation discovery, validation, and metadata reads off the
/// Slint timer. The UI coordinator submits at most one path at a time and only
/// receives a small, already formatted summary.
pub(crate) struct RecoverySummaryQueue {
    sender: SyncSender<RecoverySummaryJob>,
    completions: Receiver<RecoverySummaryCompletion>,
    in_flight: RefCell<Option<String>>,
}

impl RecoverySummaryQueue {
    pub(crate) fn start() -> Self {
        let (sender, requests) = mpsc::sync_channel::<RecoverySummaryJob>(1);
        let (completion_sender, completions) = mpsc::channel::<RecoverySummaryCompletion>();
        let worker = std::thread::Builder::new()
            .name("hirari-recovery-scan".to_owned())
            .spawn(move || {
                while let Ok(job) = requests.recv() {
                    let _worker_permit = crate::ui::worker_budget::WorkerPermit::acquire();
                    let summary = std::panic::catch_unwind(|| {
                        hirari_core_bridge::persistence::PersistenceOrchestrator::recovery_candidates(
                            &job.project_path,
                        )
                        .ok()
                        .and_then(|candidates| serde_json::to_string(&candidates).ok())
                        .map(|json| summarize_candidates(&json))
                        .unwrap_or_else(|| summarize_candidates("[]"))
                    })
                    .unwrap_or_else(|_| summarize_candidates("[]"));
                    if completion_sender
                        .send(RecoverySummaryCompletion {
                            project_path: job.project_path,
                            summary,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        if let Err(error) = worker {
            log::error!("Could not start Hirari recovery scan worker: {error}");
        }
        Self {
            sender,
            completions,
            in_flight: RefCell::new(None),
        }
    }

    pub(crate) fn request(&self, project_path: &str) {
        if project_path.trim().is_empty() || self.in_flight.borrow().is_some() {
            return;
        }
        let job = RecoverySummaryJob {
            project_path: project_path.to_owned(),
        };
        match self.sender.try_send(job) {
            Ok(()) => *self.in_flight.borrow_mut() = Some(project_path.to_owned()),
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                log::error!("Hirari recovery scan worker is unavailable");
            }
        }
    }

    pub(crate) fn poll(&self) -> Option<(String, RecoverySummary)> {
        match self.completions.try_recv() {
            Ok(completion) => {
                *self.in_flight.borrow_mut() = None;
                Some((completion.project_path, completion.summary))
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                *self.in_flight.borrow_mut() = None;
                None
            }
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RecoverySummary {
    pub count: i32,
    pub generations: Vec<i32>,
    pub text: String,
}

fn format_unix_timestamp(seconds: u64) -> String {
    const SECONDS_PER_DAY: u64 = 86_400;
    let days = seconds / SECONDS_PER_DAY;
    let day_seconds = seconds % SECONDS_PER_DAY;
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;

    // Civil date conversion from Unix days, using the proleptic Gregorian
    // calendar. Keeping it local avoids a UI-only date dependency.
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC",)
}

/// Converts the Core's validated recovery-candidate JSON into the small UI
/// model needed by the recovery dialog. Filesystem discovery remains owned by
/// the Core; this module only formats the already validated candidates.
pub(crate) fn summarize_candidates(json: &str) -> RecoverySummary {
    let candidates = serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();

    let visible = candidates.iter().take(4).collect::<Vec<_>>();
    let generations = visible
        .iter()
        .filter_map(|candidate| {
            candidate
                .get("generation")
                .and_then(serde_json::Value::as_u64)
                .map(|generation| generation.min(i32::MAX as u64) as i32)
        })
        .collect::<Vec<_>>();
    let lines = visible
        .iter()
        .filter_map(|candidate| {
            let path = candidate.get("path")?.as_str()?;
            // The Core has already validated the candidate. Prefer the
            // persisted byte count so a transient metadata read failure does
            // not hide an otherwise recoverable backup from the user.
            let bytes = candidate
                .get("bytes")
                .and_then(serde_json::Value::as_u64)
                .or_else(|| fs::metadata(path).ok().map(|metadata| metadata.len()))
                .unwrap_or_default();
            let modified = fs::metadata(path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| format_unix_timestamp(duration.as_secs()))
                .or_else(|| {
                    candidate
                        .get("modified_unix_seconds")
                        .and_then(serde_json::Value::as_u64)
                        .map(format_unix_timestamp)
                })
                .unwrap_or_else(|| "time unavailable".to_owned());
            let checksum = candidate
                .get("checksum")
                .and_then(serde_json::Value::as_u64)
                .map(|value| format!("{value:016x}"))
                .unwrap_or_else(|| "checksum unavailable".to_owned());
            Some(format!(
                "{} · {} · {} · sha {}",
                Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("backup"),
                crate::slint_ui::format_bytes(bytes),
                modified,
                checksum
            ))
        })
        .collect::<Vec<_>>();

    RecoverySummary {
        count: candidates.len().min(i32::MAX as usize) as i32,
        generations,
        text: if lines.is_empty() {
            "No readable backup metadata".to_owned()
        } else {
            lines.join("\n")
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{format_unix_timestamp, summarize_candidates};

    #[test]
    fn formats_recovery_time_for_users() {
        assert_eq!(format_unix_timestamp(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(
            format_unix_timestamp(1_735_689_600),
            "2025-01-01 00:00:00 UTC"
        );
    }

    #[test]
    fn formats_candidate_objects_and_limits_visible_generations() {
        let json = r#"[
            {"path":"/tmp/project.hirari.bak.1","generation":1,"bytes":12,"modified_unix_seconds":1735689600,"checksum":4660},
            {"path":"/tmp/project.hirari.bak.2","generation":2,"bytes":24,"modified_unix_seconds":1735689600,"checksum":4661},
            {"path":"/tmp/project.hirari.bak.3","generation":3,"bytes":36,"modified_unix_seconds":1735689600,"checksum":4662},
            {"path":"/tmp/project.hirari.bak.4","generation":4,"bytes":48,"modified_unix_seconds":1735689600,"checksum":4663},
            {"path":"/tmp/project.hirari.bak.5","generation":5,"bytes":60,"modified_unix_seconds":1735689600,"checksum":4664}
        ]"#;
        let summary = summarize_candidates(json);
        assert_eq!(summary.count, 5);
        assert_eq!(summary.generations, vec![1, 2, 3, 4]);
        assert!(summary.text.contains("project.hirari.bak.1"));
        assert!(summary.text.contains("12 B"));
        assert!(summary.text.contains("2025-01-01 00:00:00 UTC"));
        assert!(summary.text.contains("sha 0000000000001234"));
    }

    #[test]
    fn malformed_candidates_fail_closed() {
        let summary = summarize_candidates("not-json");
        assert_eq!(summary.count, 0);
        assert!(summary.generations.is_empty());
        assert_eq!(summary.text, "No readable backup metadata");
    }
}
