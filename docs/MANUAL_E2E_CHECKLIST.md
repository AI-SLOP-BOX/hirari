# Hirari manual E2E checklist

Use this checklist with a freshly built `packaging/Hirari DAW.app`. Record the
machine, macOS version, audio device, sample rate, buffer size, and date with
each run. A check is **pass** only when the visible UI state and the audible
result agree; a successful process launch alone is not sufficient.

## Startup and transport

- [ ] App opens to the initial project view without a blank or duplicated canvas.
- [ ] Play starts the playhead and audible output; Stop returns to the defined stop position.
- [ ] Pause stops audio while preserving the playhead; a second Play resumes from that position.
- [ ] Seek updates both the playhead and the audible position.
- [ ] BPM change updates the transport display and the metronome/grid timing.

## Arrangement and mixer

- [ ] Add, rename, and delete an audio track; save and reopen to confirm persistence.
- [ ] Move a region, split it, apply fade-in/fade-out and gain, then undo/redo each change.
- [ ] Volume, pan, mute, solo, and phase controls affect the rendered result.
- [ ] Save, quit, relaunch, and open the same project; confirm regions, mixer state,
      tempo, and playhead are restored.
- [ ] Edit a saved project, wait for the auto-save status, then confirm the
      ProjectDocument and piano-roll metadata reopen with the latest edit and
      an earlier matching recovery generation remains available.
- [ ] Make edits during recording and rendering; confirm auto-save waits until the
      operation ends. Simulate a read-only project folder and confirm the UI keeps
      the session marked unsaved and asks for a manual save.
- [ ] Export all stems and one selected stem from a multi-track project. Confirm
      each WAV matches its track, the aggregate progress advances, pause/resume
      works, cancel removes outputs created by the batch, and the UI remains
      responsive while preventing edits that could change later stems. Include
      track names differing only by case, Windows-reserved names, and existing
      destination files; confirm folder tracks are excluded, bus tracks can be
      rendered, outputs stay distinct, and existing files are never overwritten.
- [ ] Create a new unsaved session, edit it until the recovery-copy status appears,
      restart Hirari, and verify the recovery prompt restores the tracks and notes.
- [ ] Discard an unsaved recovery candidate and verify it is not offered again;
      choose Later and verify the candidate remains available on the next launch.

## Device boundaries

- [ ] Launch with no available output device and confirm a user-facing unavailable
      state (never a false “audio ready” state).
- [ ] Disconnect/reconnect the selected device and confirm generation/state updates
      without a crash or stale audio route.
- [ ] Repeat device reconfiguration at 44.1/48/96 kHz and 128/512-frame buffers.

## Display and accessibility

- [ ] Resize at 1080×680, 1280×800, 1600×1000, and a Retina scale factor.
- [ ] Verify keyboard focus reaches transport, track, mixer, and dialog controls.
- [ ] Verify empty, loading, failure, and recovery states provide actionable text.
- [ ] Verify text remains readable at system accessibility text-size settings.

## Evidence

Attach screenshots or a short screen recording for failures, plus the project
manifest and application log. Automated coverage is provided separately by
`scripts/run_app_launch_smoke.sh`, `scripts/run_ui_integration.sh`, and the
workspace test suite; do not mark this checklist complete from those commands
alone.
