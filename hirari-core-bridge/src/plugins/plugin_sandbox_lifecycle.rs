//! Worker lifecycle: start/stop, sandbox spawn, parameter replay, restart,
//! reconfigure and recovery policy, split out of plugin_sandbox_host.rs.

use super::*;

impl SandboxHost {
    pub(crate) fn start(&self) -> bool {
        let mut control = lock_control(&self.control);
        if !self.start_locked(&mut control) {
            return false;
        }
        // Lifecycle replay: a restarted worker starts silent, so the cached
        // control values are pushed back before audio resumes.
        self.recovery_failed.store(false, Ordering::Release);
        self.consecutive_overruns.store(0, Ordering::Release);
        if self.replay_parameters() {
            return true;
        }
        self.recovery_failed.store(true, Ordering::Release);
        self.stop_locked(&mut control);
        false
    }

    #[cfg(not(unix))]
    pub(crate) fn start_locked(&self, _control: &mut Control) -> bool {
        self.failure
            .store(FAILURE_UNSUPPORTED_PLATFORM, Ordering::Release);
        false
    }

    #[cfg(unix)]
    pub(crate) fn start_locked(&self, control: &mut Control) -> bool {
        self.failure.store(FAILURE_NONE, Ordering::Release);
        if control.plugin_path.is_empty() {
            self.failure
                .store(FAILURE_INVALID_PLUGIN_PATH, Ordering::Release);
            return false;
        }
        // Direct sandbox users share the registry's admission boundary.
        // Built-in protocol URIs are resolved by the worker and are exempt.
        if !control.plugin_path.starts_with("builtin://") {
            let expected = match control.requested_format.as_str() {
                "vst3" | "VST3" => "VST3",
                "au" | "AU" => "AU",
                "clap" | "CLAP" => "CLAP",
                _ => "",
            };
            if !crate::plugin_admission::safe_candidate(
                Path::new(&control.plugin_path),
                expected,
            ) {
                self.failure
                    .store(FAILURE_INVALID_PLUGIN_PATH, Ordering::Release);
                return false;
            }
        }
        if self.is_alive() {
            return false;
        }
        let Some(helper_path) = resolve_helper_path() else {
            self.failure
                .store(FAILURE_MISSING_HELPER, Ordering::Release);
            return false;
        };

        if !self.create_shared_memory(control) {
            self.release_shared(control);
            self.close_ipc_fds(control);
            self.failure
                .store(FAILURE_SPAWN_FAILED, Ordering::Release);
            return false;
        }

        let mut parent_to_child = [-1i32; 2];
        let mut child_to_parent = [-1i32; 2];
        if unsafe { libc::pipe(parent_to_child.as_mut_ptr()) } != 0
            || unsafe { libc::pipe(child_to_parent.as_mut_ptr()) } != 0
        {
            self.close_pipes(&mut parent_to_child, &mut child_to_parent);
            self.release_shared(control);
            self.close_ipc_fds(control);
            self.failure
                .store(FAILURE_SPAWN_FAILED, Ordering::Release);
            return false;
        }

        // Move child-bound sources above the stable target descriptors so a
        // dup2 cannot clobber a later source that the actions still need.
        let spawn_control_fd = duplicate_spawn_source(parent_to_child[0]);
        let spawn_status_fd = duplicate_spawn_source(child_to_parent[1]);
        let spawn_shared_fd = duplicate_spawn_source(control.shared_fd);
        if spawn_control_fd < 0 || spawn_status_fd < 0 || spawn_shared_fd < 0 {
            close_fd(spawn_control_fd);
            close_fd(spawn_status_fd);
            close_fd(spawn_shared_fd);
            self.close_pipes(&mut parent_to_child, &mut child_to_parent);
            self.release_shared(control);
            self.close_ipc_fds(control);
            self.failure
                .store(FAILURE_SPAWN_FAILED, Ordering::Release);
            return false;
        }

        const CONTROL_FD: i32 = 10;
        const STATUS_FD: i32 = 11;
        const SHARED_FD: i32 = 12;
        let mut actions = std::mem::MaybeUninit::<libc::posix_spawn_file_actions_t>::uninit();
        let mut actions_ok =
            unsafe { libc::posix_spawn_file_actions_init(actions.as_mut_ptr()) } == 0;
        if actions_ok {
            let actions = unsafe { actions.assume_init_mut() };
            actions_ok = unsafe {
                libc::posix_spawn_file_actions_adddup2(actions, spawn_control_fd, CONTROL_FD) == 0
                    && libc::posix_spawn_file_actions_adddup2(actions, spawn_status_fd, STATUS_FD)
                        == 0
                    && libc::posix_spawn_file_actions_adddup2(actions, spawn_shared_fd, SHARED_FD)
                        == 0
            };
        }

        let format = worker_format(&control.plugin_path, &control.requested_format);
        let mut args: Vec<CString> = Vec::with_capacity(22);
        for value in [
            helper_path.clone(),
            "--plugin".to_string(),
            control.plugin_path.clone(),
            "--format".to_string(),
            format,
            "--control-fd".to_string(),
            CONTROL_FD.to_string(),
            "--status-fd".to_string(),
            STATUS_FD.to_string(),
            "--shared-fd".to_string(),
            SHARED_FD.to_string(),
            "--shared-name".to_string(),
            control.shared_name.clone(),
            "--sample-rate".to_string(),
            // Match the previous std::to_string formatting for helpers that
            // log or compare the activation string.
            format!("{:.6}", control.sample_rate),
            "--min-frames".to_string(),
            "1".to_string(),
            "--max-frames".to_string(),
            control.max_block_size.to_string(),
            "--channels".to_string(),
            control.channels.to_string(),
        ] {
            args.push(CString::new(value).unwrap_or_default());
        }
        let mut argv: Vec<*mut c_char> = args
            .iter()
            .map(|value| value.as_ptr() as *mut c_char)
            .collect();
        argv.push(std::ptr::null_mut());
        let (environment_owned, environment) = spawn_environment();

        let mut child: libc::pid_t = -1;
        let helper_c = CString::new(helper_path).unwrap_or_default();
        let spawn_result = if actions_ok {
            unsafe {
                libc::posix_spawn(
                    &mut child,
                    helper_c.as_ptr(),
                    actions.as_ptr(),
                    std::ptr::null(),
                    argv.as_mut_ptr(),
                    environment.as_ptr(),
                )
            }
        } else {
            libc::EINVAL
        };
        drop(environment_owned);
        if actions_ok {
            unsafe {
                libc::posix_spawn_file_actions_destroy(actions.assume_init_mut());
            }
        }
        close_fd(spawn_control_fd);
        close_fd(spawn_status_fd);
        close_fd(spawn_shared_fd);

        if spawn_result != 0 {
            self.close_pipes(&mut parent_to_child, &mut child_to_parent);
            self.release_shared(control);
            self.close_ipc_fds(control);
            self.failure
                .store(FAILURE_SPAWN_FAILED, Ordering::Release);
            return false;
        }

        close_fd(parent_to_child[0]);
        close_fd(child_to_parent[1]);
        control.control_fd = parent_to_child[1];
        control.status_fd = child_to_parent[0];
        let flags = unsafe { libc::fcntl(control.status_fd, libc::F_GETFL) };
        if flags >= 0 {
            unsafe { libc::fcntl(control.status_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        }
        control.pid = child;

        let timeout_ms = ready_timeout_ms();
        if !self.wait_for_ready(control, timeout_ms) {
            self.stop_locked(control);
            if self.failure() == FAILURE_NONE {
                self.failure
                    .store(FAILURE_HANDSHAKE_TIMEOUT, Ordering::Release);
            }
            return false;
        }
        // The worker publishes its reported latency before READY, while the
        // host still owns the mapping exclusively.
        let shared = self.shared();
        let latency = if shared.is_null() {
            0
        } else {
            unsafe { (*shared).plugin_latency_samples.load(Ordering::Acquire) }
        };
        self.latency_samples.store(latency, Ordering::Release);

        self.audio_state_mut().reset_for_new_worker();
        self.process_alive.store(true, Ordering::SeqCst);
        let now = Instant::now();
        control.last_heartbeat = 0;
        control.last_completed_sequence = 0;
        control.last_heartbeat_time = now;
        control.last_progress_time = now;
        true
    }

    #[cfg(unix)]
    pub(crate) fn create_shared_memory(&self, control: &mut Control) -> bool {
        let counter = SHARED_NAME_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        control.shared_name = format!(
            "/hirari_plugin_{}_{}",
            std::process::id(),
            counter
        );
        let Ok(name) = CString::new(control.shared_name.clone()) else {
            return false;
        };
        let fd = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        if fd < 0 {
            return false;
        }
        control.shared_fd = fd;
        let length = std::mem::size_of::<SharedAudioBlock>();
        if unsafe { libc::ftruncate(fd, length as libc::off_t) } != 0 {
            return false;
        }
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            return false;
        }
        let mapped = mapped.cast::<SharedAudioBlock>();
        // Fresh shm is zero-filled; construct the protocol version and the
        // extended-MIDI ring, whose Rust-side layout the worker also uses.
        unsafe {
            (*mapped)
                .protocol_version
                .store(AUDIO_BLOCK_PROTOCOL_VERSION, Ordering::Release);
            let ring = std::ptr::addr_of_mut!((*mapped).extended_midi.bytes).cast::<c_void>();
            hirari_midi_extended_ring_init(ring, RING_STORAGE_BYTES);
        }
        control.mapped = mapped;
        control.mapped_len = length;
        self.shared.store(mapped, Ordering::Release);
        true
    }

    pub(crate) fn stop(&self) {
        let mut control = lock_control(&self.control);
        self.stop_locked(&mut control);
    }

    #[cfg(not(unix))]
    pub(crate) fn stop_locked(&self, _control: &mut Control) {
        self.process_alive.store(false, Ordering::SeqCst);
    }

    #[cfg(unix)]
    pub(crate) fn stop_locked(&self, control: &mut Control) {
        // Publish the stop before touching the mapping. The audio callback
        // observes this flag and leaves without dereferencing shared memory.
        self.process_alive.store(false, Ordering::SeqCst);
        while self.process_calls.load(Ordering::SeqCst) != 0
            || self.shared_users.load(Ordering::SeqCst) != 0
        {
            usleep(100);
        }
        if control.control_fd >= 0 {
            let command = [0xA2u8];
            unsafe {
                libc::write(control.control_fd, command.as_ptr().cast(), 1);
            }
            close_fd(control.control_fd);
            control.control_fd = -1;
        }
        if control.pid <= 0 {
            self.process_alive.store(false, Ordering::SeqCst);
            self.close_ipc_fds(control);
            self.release_shared(control);
            return;
        }
        let child = control.pid;
        unsafe {
            libc::kill(child, libc::SIGTERM);
        }
        let mut reaped = false;
        for _ in 0..100 {
            if let Some(finished) = wait_for_child(child, &mut reaped) {
                if finished {
                    reaped = true;
                    break;
                }
            }
            usleep(5000);
        }
        if !reaped {
            unsafe {
                libc::kill(child, libc::SIGKILL);
            }
            for _ in 0..100 {
                if let Some(finished) = wait_for_child(child, &mut reaped) {
                    if finished {
                        reaped = true;
                        break;
                    }
                }
                usleep(5000);
            }
        }
        if !reaped && self.failure() == FAILURE_NONE {
            self.failure
                .store(FAILURE_PROCESS_EXITED, Ordering::Release);
        }
        control.pid = -1;
        self.process_alive.store(false, Ordering::SeqCst);
        self.close_ipc_fds(control);
        self.release_shared(control);
    }

    #[cfg(unix)]
    pub(crate) fn close_ipc_fds(&self, control: &mut Control) {
        close_fd(control.control_fd);
        control.control_fd = -1;
        close_fd(control.status_fd);
        control.status_fd = -1;
    }

    #[cfg(unix)]
    pub(crate) fn close_pipes(&self, parent_to_child: &mut [i32; 2], child_to_parent: &mut [i32; 2]) {
        close_fd(parent_to_child[0]);
        close_fd(parent_to_child[1]);
        close_fd(child_to_parent[0]);
        close_fd(child_to_parent[1]);
        *parent_to_child = [-1, -1];
        *child_to_parent = [-1, -1];
    }

    #[cfg(unix)]
    pub(crate) fn release_shared(&self, control: &mut Control) {
        if !control.mapped.is_null() {
            self.shared.store(std::ptr::null_mut(), Ordering::Release);
            unsafe {
                libc::munmap(control.mapped.cast(), control.mapped_len);
            }
            control.mapped = std::ptr::null_mut();
        }
        close_fd(control.shared_fd);
        control.shared_fd = -1;
        if !control.shared_name.is_empty() {
            if let Ok(name) = CString::new(control.shared_name.clone()) {
                unsafe {
                    libc::shm_unlink(name.as_ptr());
                }
            }
            control.shared_name.clear();
        }
    }

    #[cfg(unix)]
    pub(crate) fn wait_for_ready(&self, control: &mut Control, timeout_ms: i32) -> bool {
        if control.status_fd < 0 {
            return false;
        }
        let mut descriptor = libc::pollfd {
            fd: control.status_fd,
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if result <= 0 {
            return false;
        }
        if descriptor.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            let mut status = 0;
            if control.pid > 0 && unsafe { libc::waitpid(control.pid, &mut status, libc::WNOHANG) }
                == control.pid
            {
                self.failure
                    .store(FAILURE_PROCESS_EXITED, Ordering::Release);
            }
            return false;
        }
        if descriptor.revents & libc::POLLIN == 0 {
            return false;
        }
        let mut response = 0u8;
        let read = unsafe { libc::read(control.status_fd, (&mut response as *mut u8).cast(), 1) };
        if read != 1 {
            return false;
        }
        let failure = match response {
            0xB0 => FAILURE_PLUGIN_LOAD_FAILED,
            0xB1 => FAILURE_PLUGIN_ABI_INVALID,
            0xB2 => FAILURE_PLUGIN_INSTANCE_FAILED,
            0xB3 => FAILURE_SECURITY_SETUP_FAILED,
            0xB4 => FAILURE_PLUGIN_FORMAT_UNSUPPORTED,
            0xAF => FAILURE_PLUGIN_LOAD_FAILED,
            _ => FAILURE_NONE,
        };
        if failure != FAILURE_NONE {
            self.failure.store(failure, Ordering::Release);
        }
        response == 0xA1
    }

    #[cfg(not(unix))]
    pub(crate) fn request_plugin_reset(&self) -> bool {
        false
    }

    pub(crate) fn poll_health(&self) -> bool {
        let mut control = lock_control(&self.control);
        self.poll_health_locked(&mut control)
    }

    #[cfg(not(unix))]
    pub(crate) fn poll_health_locked(&self, _control: &mut Control) -> bool {
        false
    }

    #[cfg(unix)]
    pub(crate) fn poll_health_locked(&self, control: &mut Control) -> bool {
        let child = control.pid;
        if !self.is_alive() || child <= 0 {
            return false;
        }
        let mut status = 0;
        let result = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        if result == 0 {
            let shared = self.shared();
            if !shared.is_null() {
                let shared = unsafe { &*shared };
                let heartbeat = shared.heartbeat.load(Ordering::Acquire);
                if heartbeat != control.last_heartbeat {
                    control.last_heartbeat = heartbeat;
                    control.last_heartbeat_time = Instant::now();
                }
                let requested = shared.request_sequence.load(Ordering::Acquire);
                let completed = shared.completed_sequence.load(Ordering::Acquire);
                if completed != control.last_completed_sequence {
                    control.last_completed_sequence = completed;
                    control.last_progress_time = Instant::now();
                } else if requested != completed
                    && control.last_progress_time.elapsed() > Duration::from_millis(250)
                {
                    self.failure
                        .store(FAILURE_PROCESS_HUNG, Ordering::Release);
                    self.stop_locked(control);
                    return false;
                }
            }
            return true;
        }
        self.failure
            .store(FAILURE_PROCESS_EXITED, Ordering::Release);
        control.pid = -1;
        self.process_alive.store(false, Ordering::SeqCst);
        while self.process_calls.load(Ordering::SeqCst) != 0
            || self.shared_users.load(Ordering::SeqCst) != 0
        {
            usleep(100);
        }
        self.close_ipc_fds(control);
        self.release_shared(control);
        false
    }

    pub(crate) fn restart(&self, force: bool) -> bool {
        let mut control = lock_control(&self.control);
        if !self.restart_locked(&mut control, force) {
            return false;
        }
        // A successful explicit restart is the recovery boundary: the
        // quarantine edge and the overrun streak that raced with the old
        // worker's shutdown are cleared here, not by the caller.
        self.clear_recovery_failure();
        if self.failure() == FAILURE_PLUGIN_QUARANTINED {
            control.restart_attempts = 0;
            self.failure.store(FAILURE_NONE, Ordering::Release);
        }
        if self.replay_parameters() {
            return true;
        }
        self.recovery_failed.store(true, Ordering::Release);
        self.stop_locked(&mut control);
        false
    }

    pub(crate) fn restart_locked(&self, control: &mut Control, force: bool) -> bool {
        let mut state = lock_control(&self.state);
        let _transition = TransitionGuard::enter(&self.state_transition);
        if force && self.failure() == FAILURE_PLUGIN_QUARANTINED {
            control.restart_attempts = 0;
            self.failure.store(FAILURE_NONE, Ordering::Release);
        }
        if !force
            && control
                .next_restart_time
                .is_some_and(|next| Instant::now() < next)
        {
            return false;
        }
        if self.is_alive() && !self.poll_health_locked(control) {
            return false;
        }
        let saved = self.get_state_locked(control, &mut state);
        self.stop_locked(control);
        control.restart_attempts = control.restart_attempts.saturating_add(1);
        if !self.start_locked(control) {
            let shift = (control.restart_attempts.saturating_sub(1)).min(8);
            control.next_restart_time =
                Some(Instant::now() + Duration::from_millis(100u64 << shift));
            return false;
        }
        if !saved.is_empty() && !self.set_state_locked(control, &mut state, &saved) {
            self.stop_locked(control);
            self.failure
                .store(FAILURE_PLUGIN_INSTANCE_FAILED, Ordering::Release);
            control.next_restart_time = Some(Instant::now() + Duration::from_secs(30));
            return false;
        }
        control.next_restart_time = None;
        true
    }

    pub(crate) fn auto_restart(&self) -> bool {
        const MAX_AUTOMATIC_RESTARTS: u32 = 5;
        let mut control = lock_control(&self.control);
        if control.restart_attempts >= MAX_AUTOMATIC_RESTARTS {
            self.failure
                .store(FAILURE_PLUGIN_QUARANTINED, Ordering::Release);
            return false;
        }
        if !self.restart_locked(&mut control, false) {
            return false;
        }
        self.clear_recovery_failure();
        if self.replay_parameters() {
            return true;
        }
        self.recovery_failed.store(true, Ordering::Release);
        self.stop_locked(&mut control);
        false
    }

    pub(crate) fn reconfigure(&self, sample_rate: f64, max_block_size: u32) -> bool {
        let mut control = lock_control(&self.control);
        let mut state = lock_control(&self.state);
        let _transition = TransitionGuard::enter(&self.state_transition);
        if !sample_rate.is_finite()
            || sample_rate <= 0.0
            || max_block_size == 0
            || max_block_size as usize > MAX_FRAMES
        {
            self.failure
                .store(FAILURE_PROCESS_FAILED, Ordering::Release);
            return false;
        }
        let saved = self.get_state_locked(&mut control, &mut state);
        let was_alive = self.is_alive();
        let previous_sample_rate = control.sample_rate;
        let previous_block_size = control.max_block_size;
        if was_alive {
            self.stop_locked(&mut control);
        }
        control.sample_rate = sample_rate;
        control.max_block_size = max_block_size;
        self.sample_rate_bits
            .store(sample_rate.to_bits(), Ordering::Release);
        if !was_alive {
            return true;
        }
        if !self.start_locked(&mut control) {
            // A rejected format must not strand an otherwise healthy plugin.
            control.sample_rate = previous_sample_rate;
            control.max_block_size = previous_block_size;
            self.sample_rate_bits
                .store(previous_sample_rate.to_bits(), Ordering::Release);
            if self.start_locked(&mut control)
                && !saved.is_empty()
                && !self.set_state_locked(&mut control, &mut state, &saved)
            {
                self.stop_locked(&mut control);
            }
            return false;
        }
        if !saved.is_empty() && !self.set_state_locked(&mut control, &mut state, &saved) {
            self.stop_locked(&mut control);
            self.failure
                .store(FAILURE_PLUGIN_INSTANCE_FAILED, Ordering::Release);
            control.sample_rate = previous_sample_rate;
            control.max_block_size = previous_block_size;
            self.sample_rate_bits
                .store(previous_sample_rate.to_bits(), Ordering::Release);
            if self.start_locked(&mut control)
                && !saved.is_empty()
                && !self.set_state_locked(&mut control, &mut state, &saved)
            {
                self.stop_locked(&mut control);
            }
            return false;
        }
        true
    }

    /// Device-format change for the DSP wrapper. Argument validation, the
    /// unchanged fast path, the parameter replay and the failure latch are all
    /// lifecycle policy and stay on this side of the boundary.
    pub(crate) fn prepare(&self, sample_rate: f64, max_block_size: u32) -> bool {
        if !sample_rate.is_finite() || sample_rate <= 0.0 || max_block_size == 0 {
            self.recovery_failed.store(true, Ordering::Release);
            return false;
        }
        let block_size = max_block_size.min(MAX_FRAMES as u32);
        if f64::from_bits(self.prepared_rate_bits.load(Ordering::Acquire)) == sample_rate
            && self.prepared_block.load(Ordering::Acquire) == block_size
        {
            return true;
        }
        if !self.reconfigure(sample_rate, block_size) {
            self.recovery_failed.store(true, Ordering::Release);
            return false;
        }
        self.prepared_rate_bits
            .store(sample_rate.to_bits(), Ordering::Release);
        self.prepared_block.store(block_size, Ordering::Release);
        if self.is_alive() && !self.replay_parameters() {
            self.recovery_failed.store(true, Ordering::Release);
            self.stop();
            return false;
        }
        self.recovery_failed.store(false, Ordering::Release);
        self.consecutive_overruns.store(0, Ordering::Release);
        true
    }

    pub(crate) fn replay_parameters(&self) -> bool {
        self.parameter_cache
            .visit(|parameter_id, value| self.enqueue_parameter_change(parameter_id, value, 0))
    }

    pub(crate) fn reset_recovery(&self) {
        self.recovery_failed.store(false, Ordering::Release);
        self.consecutive_overruns.store(0, Ordering::Release);
        self.clear_quarantine();
    }

    pub(crate) fn recovery_failed(&self) -> bool {
        self.recovery_failed.load(Ordering::Acquire)
    }

    pub(crate) fn clear_recovery_failure(&self) {
        self.recovery_failed.store(false, Ordering::Release);
        self.consecutive_overruns.store(0, Ordering::Release);
    }

    pub(crate) fn latch_recovery_failure(&self) {
        self.recovery_failed.store(true, Ordering::Release);
    }

    /// Realtime classification of one processed block. Returns the same flag
    /// bits as `hirari_plugin_sandbox_block_outcome`: a run of late blocks or a
    /// failed worker latches the processor, and a dead mailbox invalidates the
    /// input MIDI so it cannot leak into the recovery block.

        pub(in crate::plugin_sandbox_host) fn observe_block(&self, deadline_missed: bool, completed_late: bool, produced: bool) -> u32 {
        if deadline_missed || completed_late {
            self.consecutive_overruns.fetch_add(1, Ordering::AcqRel);
        } else if produced {
            self.consecutive_overruns.store(0, Ordering::Release);
        }
        let mut outcome = 0;
        if !self.recovery_failed() {
            if self.consecutive_overruns.load(Ordering::Acquire)
                >= crate::plugin_sandbox_status_text::CONSECUTIVE_OVERRUN_LIMIT
                || (!produced && self.failure() == FAILURE_PROCESS_FAILED)
            {
                outcome |= crate::plugin_sandbox_status_text::OUTCOME_LATCH_FAILURE;
                self.recovery_failed.store(true, Ordering::Release);
            }
        }
        if !produced
            && matches!(
                self.failure(),
                FAILURE_PLUGIN_QUARANTINED
                    | FAILURE_PROCESS_EXITED
                    | FAILURE_PROCESS_HUNG
                    | FAILURE_PROCESS_FAILED
            )
        {
            outcome |= crate::plugin_sandbox_status_text::OUTCOME_CLEAR_MIDI;
        }
        outcome
    }
}
