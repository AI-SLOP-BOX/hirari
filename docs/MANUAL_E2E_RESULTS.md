# Manual E2E execution record

## 2026-09-04

Manual Computer Use pass completed against the freshly rebuilt
`packaging/Aura DAW.app` on macOS. The app launched with the Quick Start
surface, the Electronic template produced its five-track project, and the
Piano Roll and Mixer views opened without leaving a blank canvas. The Help
route opened Diagnostics and displayed the Slint `AboutSlint` attribution.
No crash, stale-window, or accessibility-action failure was observed.

This pass validates the startup/template/navigation/attribution path only; it
does not certify every audio device, plug-in vendor, or multi-hour session.

## 2026-08-29

The manual UI pass was attempted against the freshly built
`packaging/Aura DAW.app`, but the host Mac was locked and Computer Use could
not unlock it. No UI assertion is marked as passed from that attempt.

After unlocking the Mac, run the checklist in
[`MANUAL_E2E_CHECKLIST.md`](MANUAL_E2E_CHECKLIST.md) and record the host,
display scale, audio device, sample rate, buffer size, and screenshots for any
failure. Automated launch, project, render, device-boundary, and UI/Core tests
are separate evidence and do not replace this manual pass.
