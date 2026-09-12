#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum QuickExportFormat {
    Wav,
    Mp3,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectEventRange {
    pub start_sample: u64,
    pub end_sample: u64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct QuickExportRequest {
    pub project_name: String,
    pub format: QuickExportFormat,
    pub project_sample_rate: u32,
    pub event_ranges: Vec<ProjectEventRange>,
    pub effect_tail_samples: u64,
    pub main_mix: AvailableExportChannel,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct QuickExportPlan {
    pub channel_id: u32,
    pub start_sample: u64,
    pub end_sample: u64,
    pub filename: String,
    pub codec: CodecRust,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub bitrate_kbps: Option<u16>,
    pub realtime: bool,
}

/// Builds Cubase-style Quick Audio Export settings for the Main Mix. The
/// caller supplies project event bounds, keeping project/Arranger ownership
/// outside the export engine.
pub fn plan_quick_export(request: &QuickExportRequest) -> Result<QuickExportPlan, String> {
    if request.project_name.trim().is_empty() || request.project_name.len() > 256
        || unsafe_name(&request.project_name) || !(8_000..=384_000).contains(&request.project_sample_rate)
        || request.main_mix.kind != ExportChannelKind::Output || !request.main_mix.validate()
        || request.event_ranges.is_empty() || request.event_ranges.len() > 1_000_000
        || request.event_ranges.iter().any(|range| range.start_sample >= range.end_sample) {
        return Err("quick export request is invalid".into());
    }
    let start_sample = request.event_ranges.iter().map(|range| range.start_sample).min().unwrap();
    let content_end = request.event_ranges.iter().map(|range| range.end_sample).max().unwrap();
    let max_tail = u64::from(request.project_sample_rate)
        .checked_mul(30).ok_or_else(|| "quick export tail overflow".to_owned())?;
    let end_sample = content_end.checked_add(request.effect_tail_samples.min(max_tail))
        .ok_or_else(|| "quick export range overflow".to_owned())?;
    let stem = sanitize_export_name(&request.project_name);
    if stem.is_empty() { return Err("quick export filename is invalid".into()); }
    let (codec, sample_rate, bit_depth, bitrate_kbps, extension) = match request.format {
        QuickExportFormat::Wav => (CodecRust::Wav, request.project_sample_rate, 24, None, "wav"),
        QuickExportFormat::Mp3 => (CodecRust::Mp3, 44_100, 16, Some(256), "mp3"),
    };
    Ok(QuickExportPlan {
        channel_id: request.main_mix.id,
        start_sample,
        end_sample,
        filename: format!("{stem}.{extension}"),
        codec,
        sample_rate,
        bit_depth,
        bitrate_kbps,
        realtime: request.main_mix.requires_realtime,
    })
}

impl QuickExportPlan {
    pub fn validate(&self) -> bool {
        self.channel_id != 0 && self.start_sample < self.end_sample && !self.filename.is_empty()
            && self.filename.len() <= 1024 && !unsafe_name(&self.filename)
            && match self.codec {
                CodecRust::Wav => self.filename.ends_with(".wav") && (8_000..=384_000).contains(&self.sample_rate)
                    && self.bit_depth == 24 && self.bitrate_kbps.is_none(),
                CodecRust::Mp3 => self.filename.ends_with(".mp3") && self.sample_rate == 44_100
                    && self.bit_depth == 16 && self.bitrate_kbps == Some(256),
                _ => false,
            }
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExportQueueStatus { Pending, Rendering, Completed, Failed, Cancelled }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExportQueueJobPro { pub id: u64, pub plan: ExportPlanPro, pub status: ExportQueueStatus, pub error: Option<String> }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExportQueuePro { pub jobs: Vec<ExportQueueJobPro>, next_id: u64 }

impl Default for ExportQueuePro { fn default() -> Self { Self { jobs: Vec::new(), next_id: 1 } } }

impl ExportQueuePro {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit() { return Err("invalid export queue".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let mut value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        for job in &mut value.jobs {
            if job.status == ExportQueueStatus::Rendering {
                job.status = ExportQueueStatus::Pending;
                job.error = None;
            }
        }
        if value.audit() { Ok(value) } else { Err("invalid export queue".into()) }
    }

    pub fn enqueue(&mut self, plan: ExportPlanPro) -> Option<u64> {
        if !plan.validate() || self.jobs.len() >= 20 { return None; }
        let id = self.next_id; self.next_id = self.next_id.checked_add(1)?;
        self.jobs.push(ExportQueueJobPro { id, plan, status: ExportQueueStatus::Pending, error: None }); Some(id)
    }
    pub fn move_job(&mut self, id: u64, new_index: usize) -> bool {
        let Some(old_index) = self.jobs.iter().position(|job| job.id == id && job.status == ExportQueueStatus::Pending) else { return false; };
        if new_index >= self.jobs.len() { return false; }
        let job = self.jobs.remove(old_index); self.jobs.insert(new_index, job); true
    }
    pub fn update_job(&mut self, id: u64, plan: ExportPlanPro) -> bool {
        if !plan.validate() { return false; }
        let Some(job) = self.jobs.iter_mut().find(|job| job.id == id && job.status == ExportQueueStatus::Pending) else { return false; };
        job.plan = plan; job.error = None; true
    }
    pub fn remove_job(&mut self, id: u64) -> bool {
        let Some(index) = self.jobs.iter().position(|job| job.id == id
            && matches!(job.status, ExportQueueStatus::Pending | ExportQueueStatus::Failed | ExportQueueStatus::Cancelled)) else { return false; };
        self.jobs.remove(index); true
    }
    pub fn remove_all(&mut self) -> bool {
        if self.jobs.iter().any(|job| job.status == ExportQueueStatus::Rendering) { return false; }
        self.jobs.clear(); true
    }
    pub fn begin(&mut self, id: u64) -> bool { self.transition(id, ExportQueueStatus::Pending, ExportQueueStatus::Rendering, None) }
    pub fn complete(&mut self, id: u64) -> bool { self.transition(id, ExportQueueStatus::Rendering, ExportQueueStatus::Completed, None) }
    pub fn fail(&mut self, id: u64, error: &str) -> bool {
        if error.trim().is_empty() || error.len() > 2048 || error.contains('\0') { return false; }
        self.transition(id, ExportQueueStatus::Rendering, ExportQueueStatus::Failed, Some(error.trim().to_owned()))
    }
    pub fn retry(&mut self, id: u64) -> bool { self.transition(id, ExportQueueStatus::Failed, ExportQueueStatus::Pending, None) }
    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(job) = self.jobs.iter_mut().find(|job| job.id == id
            && matches!(job.status, ExportQueueStatus::Pending | ExportQueueStatus::Failed)) else { return false; };
        job.status = ExportQueueStatus::Cancelled; job.error = None; true
    }
    pub fn next_pending(&self) -> Option<u64> {
        self.jobs.iter().find(|job| job.status == ExportQueueStatus::Pending).map(|job| job.id)
    }
    pub fn retry_all_failed(&mut self) -> usize {
        let mut count = 0;
        for job in &mut self.jobs {
            if job.status == ExportQueueStatus::Failed { job.status = ExportQueueStatus::Pending; job.error = None; count += 1; }
        }
        count
    }
    pub fn counts(&self) -> (usize, usize, usize, usize, usize) {
        let mut counts = [0usize; 5];
        for job in &self.jobs { counts[match job.status { ExportQueueStatus::Pending => 0, ExportQueueStatus::Rendering => 1, ExportQueueStatus::Completed => 2, ExportQueueStatus::Failed => 3, ExportQueueStatus::Cancelled => 4 }] += 1; }
        (counts[0], counts[1], counts[2], counts[3], counts[4])
    }
    pub fn audit(&self) -> bool {
        self.jobs.len() <= 20 && self.next_id > 0
            && self.jobs.iter().enumerate().all(|(index, job)| job.id > 0 && job.id < self.next_id
                && job.plan.validate() && self.jobs[..index].iter().all(|previous| previous.id != job.id)
                && match job.status {
                    ExportQueueStatus::Failed => job.error.as_ref().is_some_and(|error| !error.trim().is_empty()
                        && error.len() <= 2048 && !error.contains('\0')),
                    _ => job.error.is_none(),
                })
    }
    fn transition(&mut self, id: u64, from: ExportQueueStatus, to: ExportQueueStatus, error: Option<String>) -> bool {
        let Some(job) = self.jobs.iter_mut().find(|job| job.id == id && job.status == from) else { return false; };
        job.status = to; job.error = error; true
    }
}

/// Tracks only files produced by the current queue run. Cancellation delegates
/// deletion to the host, then restores jobs to Pending so the saved queue is
/// retained exactly as Cubase does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportQueueRun { published: std::collections::BTreeMap<u64, Vec<PathBuf>> }

impl ExportQueueRun {
    pub fn record_published(&mut self, queue: &ExportQueuePro, job_id: u64, path: PathBuf) -> bool {
        if path.as_os_str().is_empty() || path.file_name().is_none()
            || !queue.jobs.iter().any(|job| job.id == job_id && job.status == ExportQueueStatus::Rendering) { return false; }
        let paths = self.published.entry(job_id).or_default();
        if paths.len() >= 1_000_000 || paths.contains(&path) { return false; }
        paths.push(path); true
    }

    pub fn cancel_with<F>(&mut self, queue: &mut ExportQueuePro, mut remove: F) -> Result<usize, String>
    where F: FnMut(&Path) -> Result<(), String> {
        let mut removed = 0usize;
        while let Some((job_id, path)) = self.published.iter().find_map(|(job_id, paths)| paths.first().cloned().map(|path| (*job_id, path))) {
            remove(&path)?;
            let paths = self.published.get_mut(&job_id).expect("published job exists");
            paths.remove(0); removed += 1;
            if paths.is_empty() { self.published.remove(&job_id); }
        }
        for job in &mut queue.jobs {
            if matches!(job.status, ExportQueueStatus::Rendering | ExportQueueStatus::Completed) {
                job.status = ExportQueueStatus::Pending; job.error = None;
            }
        }
        Ok(removed)
    }

    pub fn published_count(&self) -> usize { self.published.values().map(Vec::len).sum() }
}

#[cfg(test)]
mod export_plan_tests {
    use super::*;

    fn request() -> ExportRequestPro {
        ExportRequestPro { project_name: "Album".into(), channel_ids: vec![1, 2],
            range: ExportRangeSelection::CycleMarkers(vec![NamedExportRange { id: 1, name: "Verse".into(),
                start_sample: 0, end_sample: 48_000 }, NamedExportRange { id: 2, name: "Chorus".into(),
                start_sample: 48_000, end_sample: 96_000 }]), codec: CodecRust::Wav, sample_rate: 48_000,
            bit_depth: 24, effects: ExportEffectsMode::MasterGroupsAndSends,
            channel_mode: ExportChannelMode::Interleaved, naming: ExportNamingScheme {
                parts: vec![NamingPart::Project, NamingPart::Channel, NamingPart::Range], separator: "_".into() },
            realtime: false, deactivate_external_midi: true, existing_file_policy: ExistingFilePolicy::Error }
    }
    fn channels() -> Vec<AvailableExportChannel> { vec![
        AvailableExportChannel { id: 1, name: "Drums".into(), kind: ExportChannelKind::Group,
            channels: 2, selected: true, requires_realtime: false },
        AvailableExportChannel { id: 2, name: "Hardware Synth".into(), kind: ExportChannelKind::Instrument,
            channels: 2, selected: true, requires_realtime: true }] }

    #[test]
    fn plans_channel_by_range_matrix_and_detects_realtime() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        assert_eq!(plan.files.len(), 4); assert!(plan.realtime);
        assert_eq!(plan.files[0].filename, "Album_Drums_Verse.wav");
        assert!(plan.files.iter().all(|file| file.output_channels == 2 && file.realtime));
        assert_eq!(plan.total_source_samples, 192_000);
    }

    #[test]
    fn split_channels_and_increment_policy_never_collide() {
        let mut request = request(); request.channel_ids = vec![1]; request.range = ExportRangeSelection::Locators {
            start_sample: 0, end_sample: 48_000 }; request.channel_mode = ExportChannelMode::SplitChannels;
        request.naming.parts = vec![NamingPart::Literal("Stem".into())];
        request.existing_file_policy = ExistingFilePolicy::IncrementName;
        let plan = plan_export(&request, &channels(), &["Stem_ch01.wav".into()]).unwrap();
        assert_eq!(plan.files.iter().map(|file| file.filename.as_str()).collect::<Vec<_>>(),
            vec!["Stem_ch01_2.wav", "Stem_ch02.wav"]);
    }

    #[test]
    fn invalid_range_and_lr_from_mono_fail_before_rendering() {
        let mut request = request(); request.channel_ids = vec![1];
        request.range = ExportRangeSelection::Locators { start_sample: 10, end_sample: 10 };
        assert!(plan_export(&request, &channels(), &[]).is_err());
        request.range = ExportRangeSelection::Locators { start_sample: 0, end_sample: 10 };
        request.channel_mode = ExportChannelMode::LeftRightFromSurround;
        let mono = vec![AvailableExportChannel { channels: 1, ..channels()[0].clone() }];
        assert!(plan_export(&request, &mono, &[]).is_err());
    }

    #[test]
    fn overwrite_allows_disk_replacement_but_not_duplicate_plan_outputs() {
        let mut request = request();
        request.channel_ids = vec![1];
        request.range = ExportRangeSelection::Locators {
            start_sample: 0,
            end_sample: 48_000,
        };
        request.naming.parts = vec![NamingPart::Literal("Mix".into())];
        request.existing_file_policy = ExistingFilePolicy::Overwrite;
        let plan = plan_export(&request, &channels(), &["Mix.wav".into()]).unwrap();
        assert_eq!(plan.files[0].filename, "Mix.wav");

        request.channel_ids = vec![1, 2];
        assert!(plan_export(&request, &channels(), &[]).is_err());
    }

    #[test]
    fn quick_wav_uses_main_mix_project_rate_and_caps_tail_at_thirty_seconds() {
        let request = QuickExportRequest {
            project_name: "Film Score".into(),
            format: QuickExportFormat::Wav,
            project_sample_rate: 48_000,
            event_ranges: vec![
                ProjectEventRange { start_sample: 24_000, end_sample: 96_000 },
                ProjectEventRange { start_sample: 0, end_sample: 48_000 },
            ],
            effect_tail_samples: 48_000 * 45,
            main_mix: AvailableExportChannel { id: 9, name: "Main Mix".into(),
                kind: ExportChannelKind::Output, channels: 2, selected: true, requires_realtime: true },
        };
        let plan = plan_quick_export(&request).unwrap();
        assert_eq!(plan.start_sample, 0);
        assert_eq!(plan.end_sample, 96_000 + 48_000 * 30);
        assert_eq!(plan.filename, "Film Score.wav");
        assert_eq!(plan.sample_rate, 48_000);
        assert_eq!(plan.bit_depth, 24);
        assert!(plan.realtime);
        assert!(plan.validate());
    }

    #[test]
    fn quick_mp3_uses_fixed_delivery_format_and_rejects_non_output_channel() {
        let mut request = QuickExportRequest {
            project_name: "Demo".into(),
            format: QuickExportFormat::Mp3,
            project_sample_rate: 96_000,
            event_ranges: vec![ProjectEventRange { start_sample: 100, end_sample: 1_000 }],
            effect_tail_samples: 0,
            main_mix: AvailableExportChannel { id: 4, name: "Main Mix".into(),
                kind: ExportChannelKind::Output, channels: 2, selected: true, requires_realtime: false },
        };
        let plan = plan_quick_export(&request).unwrap();
        assert_eq!((plan.sample_rate, plan.bit_depth, plan.bitrate_kbps), (44_100, 16, Some(256)));
        assert_eq!(plan.filename, "Demo.mp3");
        assert!(!plan.realtime);
        assert!(plan.validate());

        request.main_mix.kind = ExportChannelKind::Group;
        assert!(plan_quick_export(&request).is_err());
        request.main_mix.kind = ExportChannelKind::Output;
        request.event_ranges[0].end_sample = request.event_ranges[0].start_sample;
        assert!(plan_quick_export(&request).is_err());
    }

    #[test]
    fn export_queue_has_explicit_retryable_transitions() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default(); let id = queue.enqueue(plan).unwrap();
        assert!(queue.begin(id)); assert!(queue.fail(id, "encoder unavailable")); assert!(queue.retry(id));
        assert!(queue.begin(id)); assert!(queue.complete(id));
        assert_eq!(queue.jobs[0].status, ExportQueueStatus::Completed);
    }

    #[test]
    fn export_queue_reports_counts_and_retries_all_failures() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        let first = queue.enqueue(plan.clone()).unwrap();
        let second = queue.enqueue(plan).unwrap();
        assert!(queue.begin(first));
        assert!(queue.fail(first, "temporary"));
        assert!(queue.begin(second));
        assert!(queue.fail(second, "temporary"));
        assert_eq!(queue.counts(), (0, 0, 0, 2, 0));
        assert_eq!(queue.retry_all_failed(), 2);
        assert_eq!(queue.next_pending(), Some(first));
        assert!(queue.audit());
    }

    #[test]
    fn export_queue_round_trip_recovers_interrupted_render_as_pending() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        let id = queue.enqueue(plan).unwrap();
        assert!(queue.begin(id));
        let json = queue.to_json().unwrap();
        let restored = ExportQueuePro::from_json(&json).unwrap();
        assert_eq!(restored.jobs[0].status, ExportQueueStatus::Pending);
        assert_eq!(restored.next_pending(), Some(id));
        assert!(restored.audit());
    }

    #[test]
    fn export_queue_rejects_tampered_plan_totals_and_duplicate_outputs() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        queue.enqueue(plan).unwrap();
        let json = queue.to_json().unwrap();
        let invalid_total = json.replace("\"total_source_samples\":192000", "\"total_source_samples\":1");
        assert!(ExportQueuePro::from_json(&invalid_total).is_err());
        let mut duplicate = queue.clone();
        duplicate.jobs[0].plan.files[1].filename = duplicate.jobs[0].plan.files[0].filename.clone();
        assert!(duplicate.to_json().is_err());
    }

    #[test]
    fn queue_enforces_cubase_twenty_job_limit_and_updates_pending_job() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        for _ in 0..20 { assert!(queue.enqueue(plan.clone()).is_some()); }
        assert!(queue.enqueue(plan.clone()).is_none());
        let id = queue.jobs[0].id;
        let mut updated = plan; updated.files[0].filename = "Updated.wav".into();
        assert!(queue.update_job(id, updated));
        assert_eq!(queue.jobs[0].plan.files[0].filename, "Updated.wav");
        assert!(queue.begin(id));
        assert!(!queue.update_job(id, queue.jobs[0].plan.clone()));
        assert!(queue.audit());
    }

    #[test]
    fn cancelling_run_removes_only_recorded_outputs_and_keeps_jobs_pending() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        let first = queue.enqueue(plan.clone()).unwrap();
        let second = queue.enqueue(plan).unwrap();
        assert!(queue.begin(first));
        let mut run = ExportQueueRun::default();
        assert!(run.record_published(&queue, first, PathBuf::from("/mixdown/one.wav")));
        assert!(run.record_published(&queue, first, PathBuf::from("/mixdown/two.wav")));
        assert!(!run.record_published(&queue, second, PathBuf::from("/mixdown/not-rendering.wav")));
        assert!(queue.complete(first));
        assert!(queue.begin(second));
        let mut removed = Vec::new();
        assert_eq!(run.cancel_with(&mut queue, |path| { removed.push(path.to_path_buf()); Ok(()) }).unwrap(), 2);
        assert_eq!(removed.len(), 2);
        assert!(queue.jobs.iter().all(|job| job.status == ExportQueueStatus::Pending));
        assert_eq!(run.published_count(), 0);
        assert!(queue.audit());
    }

    #[test]
    fn remove_all_refuses_to_mutate_a_running_queue() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        let id = queue.enqueue(plan).unwrap();
        assert!(queue.begin(id));
        assert!(!queue.remove_all());
        assert_eq!(queue.jobs.len(), 1);
    }
}
