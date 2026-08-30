# Manual E2E execution record

## 2026-08-29

The manual UI pass was attempted against the freshly built
`packaging/Aura DAW.app`, but the host Mac was locked and Computer Use could
not unlock it. No UI assertion is marked as passed from that attempt.

After unlocking the Mac, run the checklist in
[`MANUAL_E2E_CHECKLIST.md`](MANUAL_E2E_CHECKLIST.md) and record the host,
display scale, audio device, sample rate, buffer size, and screenshots for any
failure. Automated launch, project, render, device-boundary, and UI/Core tests
are separate evidence and do not replace this manual pass.
