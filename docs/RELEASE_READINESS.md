# Release readiness

アプリバンドル生成はステージング先で完了・署名してから原子置換する。コンパイルや署名が途中で失敗しても、直前の既知のバンドルを先に削除しない。

## 現行ツリー再検証（2026-08-29 追加）

- `cargo check --workspace --locked`: 成功
- `cargo test --workspace --all-targets --locked -- --test-threads=1`: 成功（失敗 0）
- `scripts/build_app.sh`: 成功（release bundle を生成）
- `codesign --verify --deep --strict`: 成功
- GUI launch smoke: 成功
- `AURA_HEADLESS=1` 起動: `native_engine=ready`, `audio_driver=running`
- FFmpeg codec contract (hostile Unicode paths, MP3/FLAC): 成功
- 100-track realtime matrix (48 kHz, 128/256/512/1024 frames, 2,000 blocks): deadline miss 0
- Recording stress / cancellation / recovery gate: 成功
- Software device transition contract: 成功
- macOS CoreAudio initialize/start/stop/reconfigure contract: 成功
- `cargo fmt --all -- --check`: 成功
- Release metadata ZIP + SHA-256 verification: 成功
- `scripts/setup_dev.sh` (現行Rust/Xcode/CMake/FFmpeg環境): 成功
- `scripts/run_clean_build.sh` (隔離ターゲットでfetch/check/全テスト): 成功
- AU実機E2E（Apple auval + Surge XT worker、状態復元・再構成・過負荷隔離）: 成功
- Release realtime callback soak (ignored long-duration test, 30 s): 成功、deadline miss 0
- Manual UI E2E: 未実行（ホストMacのロックにより自動操作不可、詳細は [`MANUAL_E2E_RESULTS.md`](MANUAL_E2E_RESULTS.md)）

## 現行ツリー再検証（2026-08-29）

現行の作業ツリーに対して、整形チェック、workspaceコンパイル、全workspace
テスト、UI/Core統合スモーク、リポジトリ健全性、リリース成果物のチェックサム検証、
`cargo audit`、`cargo deny check advisories licenses bans sources` を再実行し、
いずれも成功した。CoreAudioデバイス契約も `PASS` となった。

プラグインインベントリでは、AU（Surge XT、Vital、ASAF）、VST3（Surge XT、Vital）、
CLAP（Surge XT、Vital）のUniversalバンドルを検出した。ただし、これは存在・形式・
アーキテクチャの確認であり、GUI埋め込みや状態復元後の音声一致の証明ではない。

OpenUtauの完全往復は、現環境に実レンダラーのコマンド指定がないため `SKIPPED`。
VST3はSDK未準備、AUは外部コンポーネント検証が時間内に完了しなかったため、いずれも
成功扱いにはしていない。

Aura is suitable for public source release as an early technical preview.
This document defines what Aura may claim and what requires an environment
specific evidence artifact.

## Capability matrix

| Area | Public claim | Evidence required for a release claim |
| --- | --- | --- |
| CLAP | Supported through the isolated worker path | Packaged worker smoke and CLAP fixture E2E |
| AU | Supported on macOS when the worker reports AU capability | macOS AU fixture/component E2E |
| VST3 | Supported only by an SDK-enabled build | Official Steinberg SDK, SDK-enabled worker, fixture E2E |
| MP3/FLAC import/export | Supported through the allowlisted FFmpeg boundary when FFmpeg is available | hostile-path codec import/export contract and release-environment FFmpeg check |
| Vital | Compatible host target, not bundled; VST3/AU/CLAP evidence is format-specific | Universal Vital AU/VST3/CLAP binaries are now inventoried on the current Mac; instantiate, reconfigure, state restore, crash/quarantine, overrun recovery, and full project save/restart/render checks remain unverified. |
| Surge XT | Compatible host target, not bundled | Real Surge XT instance in the selected format and architecture |
| OpenUtau | Bridge/import integration; external render handoff | OpenUtau project import, tuning edit, render, re-import and audio comparison |
| Long-running stability | Testable, not universally guaranteed | Device-specific duration run with zero unexplained dropouts, NaN/Inf, leaks or corruption |
| Forkability | Aura-owned source can be forked under MIT | Clean checkout build and repository health gate |

The plugin rows do not imply redistribution. Plugin binaries, presets,
voicebanks and SDKs remain subject to their own licenses and installation
requirements.

The latest local inventory (`scripts/run_plugin_compatibility_matrix.sh`) found
universal Surge XT and Vital binaries in AU, VST3, and CLAP locations, plus
universal ASAF AU effects. This confirms availability and architecture only;
it does not by itself prove instance processing, GUI embedding, preset/state
restore, or rendered-audio equivalence for those vendors.

## Current local evidence (2026-08-29)

The current worktree was rebuilt with `scripts/build_app.sh`. The packaged
binary passed direct headless smoke and strict ad-hoc signature validation:

```text
AURA_HEADLESS_READY native_engine=ready bridge=ready project_layout=valid resources=ready audio_device_ready=true audio_driver=running
packaging/Aura DAW.app: valid on disk
```

The same bundle was launched through the normal macOS application path; a
window process was observed and then exited cleanly. This proves launch/exit
only, not manual transport or device interaction.

The packaged app was rebuilt from the current worktree on 2026-08-29 and
revalidated with the bundled executable's headless readiness probe and
`codesign --verify --deep --strict`.

The broader bundle verifier still requires external plugin fixtures. The CLAP
fixture instantiate probe hung in this environment and was stopped; it remains
unresolved plugin evidence, not an application build failure.

The FFmpeg boundary contract was re-run on 2026-08-29 and passed for MP3 and
FLAC conversion, including hostile input/output paths containing spaces,
semicolons, and shell-like text.

The current real-device matrix reports the software device transition and
CoreAudio device contract as passing. The installed AU fixture timed out under
the bounded 14-second probe, and the VST3 fixture was skipped because
`AURA_VST3_SDK` is not configured. These are recorded as unresolved external
fixture evidence, not promoted to application failures or compatibility claims.

## Strict release gate

The canonical entry point is:

```sh
cmake --build build --target aura-verify-release
```

On macOS, `aura-verify` now depends on that same strict release target before
running repository-health checks. There is no weaker CMake verification path
that can pass while the packaged release gate is failing.

The ordinary Rust/UI suite runs with `AURA_NATIVE_TEST_ISOLATION=1` so its
results are deterministic and do not silently depend on whichever CoreAudio
device happens to be connected. Real device initialization, start/stop and
reconfiguration are exercised separately by the strict device-transition gate
owned by `aura-verify-release`.

The same lockfile-enforced formatting, workspace check, and serialized test
commands also run in `.github/workflows/ci.yml` on every pull request and push
to `main`, providing a clean-environment reproducibility gate. The workflow
also builds the macOS bundle on `macos-14`, runs its headless readiness probe,
and verifies the ad-hoc signature.

That target builds the app, creates the repository CLAP fixture, runs the
full test groups, and verifies the packaged bundle. On macOS it also requires
the configured device and external-plugin matrix. Missing SDKs, fixtures or
device capabilities are not silently promoted to a pass in strict mode.
The bundle build also emits `Contents/Resources/aura-resources.manifest`; the
headless app check and release verifier require this readable manifest instead
of treating an empty `Resources` directory as proof that packaging succeeded.

The full test group includes `aura-test-cli-e2e`, which creates a canonical
project, queries its generation pair, performs a native JSONL bounce, verifies
the resulting WAV, replays the same request through the durable ledger, and
checks history branch restore. The ledger is written as `prepared` before
native mutation and refuses an in-flight retry after a simulated crash, then
publishes `committed` only after the result is durable. This proves the CLI
path is connected to the native render path rather than only validating
command syntax, and that at-least-once transports cannot silently duplicate a
mutation. History restore also verifies the working file's persistent project
UUID before replacing it, so a different project copied into the same path is
rejected without modification.

The sandbox contract also includes a deliberate state-callback hang fixture.
When a plugin exceeds the bounded state deadline, the host marks it
`ProcessHung`, detaches it from the audio graph without unmapping shared memory
while the request is unwinding, and leaves reaping to the serialized lifecycle
cleanup path. The regression test verifies finite fallback audio and a dead
worker status instead of treating the timeout as a successful restore.

For a repeatable local hardware/plugin evidence run, use the matrix runner:

```sh
scripts/run_real_device_matrix.sh
```

It writes a timestamped TSV report and one log per capability under
`$AURA_EVIDENCE_DIR` (or the system temporary directory). The report keeps
`PASS`, `FAIL`, and `SKIPPED` distinct; set `AURA_REAL_DEVICE_STRICT=1` to
make missing hardware, SDKs, or third-party fixtures fail the run instead of
allowing an incomplete matrix. This runner is evidence collection, not a
universal compatibility claim.

For a VST3-capable build, configure the official SDK and fixture explicitly:

```sh
AURA_VST3_SDK=/path/to/vst3sdk \
AURA_VST3_FIXTURE=/path/to/plugin.vst3 \
cmake --build build --target aura-verify-release
```

## Latest local evidence

### Current worktree software evidence (2026-08-29)

The current uncommitted worktree has been rebuilt with Rust 1.98.0 and the
tracked lockfile. `cargo check --workspace --locked`, the serialized full
workspace test suite, `scripts/run_ui_integration.sh`, dependency vulnerability
and license policy gates, the native compile contract, and CycloneDX SBOM plus
SHA-256 metadata generation passed. The UI integration run included the real
Slint-to-Core production workflow and the repository CLAP worker's instantiate,
continuous processing, crash/quarantine, and overrun recovery paths.

`scripts/run_realtime_performance_matrix.sh` also measured 100 native tracks at
48 kHz for 2,000 post-warmup blocks per buffer size. There were zero callback
deadline misses. The latest observed maximum / p99 times were 589 / 398 us at 128
frames, 621 / 613 us at 256, 1,097 / 1,070 us at 512, and 1,956 / 1,926 us at
1,024. These are software graph measurements on this Apple Silicon host; they
do not replace CoreAudio device, sleep/wake, physical MIDI, or long-duration
hardware evidence.

The repository remains dirty, so this section is not a release approval. The
strict matrix must still be rerun after the changes are intentionally staged
into reviewable commits, and distribution signing/notarization requires release
credentials.

The current source was also rebuilt into `packaging/Aura DAW.app`. Bundle
structure, executable hashes, resources, ad-hoc signature, packaged CLAP worker,
and headless application initialization all passed with plugin retries forbidden.
The previously captured machine matrix report
`$TMPDIR/aura-real-device-evidence-current/matrix-20260828-230314.tsv` recorded
zero skips: software device transition, attached CoreAudio initialize/start/
stop/reconfigure, Surge XT VST3, Surge XT AU, and repository CLAP all passed.

Audio-file compatibility is covered by current workspace contracts for PCM
16/24/32-bit WAV, mono and multichannel interleaving, 44.1/48/96 kHz sample
rates (including simultaneous export), Unicode paths, empty/corrupt input
rejection, and unique-filename Missing-asset relinking. These contracts prove
parser/export behavior; external-drive I/O and long-running UI responsiveness
still require device-specific evidence.

The current worktree was revalidated on 2026-08-29 with the serialized
workspace suite: 468 core unit tests, 2 editing, 3 export, 2 MIDI, 7 recording,
9 render, 11 routing, 1 undo, 33 UI, and 4 UI/Core integration tests passed.
Fixture-dependent external-plugin tests remain explicitly skipped unless their
SDKs and binaries are installed; no skip is counted as a plugin compatibility
pass.
The real plugin runs covered instrument audio, state restore, audio
reconfiguration, crash/quarantine, and overrun recovery. This remains
machine-specific evidence and does not imply compatibility with every vendor or
architecture.

The following 2026-08-27 macOS release gate is retained as historical evidence
(it does not describe the current external-plugin run):
capability inputs:

| Gate | Result | Evidence input |
| --- | --- | --- |
| Full `aura-verify-release` | Passed | release app, full Rust/UI tests, ASan/UBSan, TSan, stress and bundle verification |
| CLAP worker | Passed | repository `minimal-gain.clap` fixture, packaged worker handshake/process/recovery |
| VST3 worker | Passed | Steinberg SDK at `/tmp/aura-vst3-sdk-new` and installed Surge XT VST3 fixture; process, bounded MIDI, state restore, crash/quarantine and overrun recovery smoke |
| AU worker | Passed | installed Surge XT component; Apple AU validation plus isolated effect initialization, ClassInfo state round-trip, MIDI forwarding and reconfiguration smoke |
| CoreAudio device transition | Passed | initialize/start/stop/reconfigure contract on the attached macOS device |
| OpenUtau project/CLI handoff | Passed (local evidence) | OpenUtau v0.1.565.0 opened `teto_chorus_aura_tuned_diagnostic_v10.ustx` with Kasane Teto / WORLDLINE-R; Aura CLI imported the same USTX plus a WAV, persisted the pair, and re-inspected 16 source notes, singer identity, SHA-256 hashes, 44.1 kHz stereo audio, 8,464,113 frames and one region |
| Recording lifecycle | Passed (contract evidence) | RIFF/RF64-capable header finalization, exact finalized frame-count verification across the writer drain boundary, WAV re-read after stop, repeated same-path take replacement and 250-round lifecycle/temporary-file cleanup stress; `aura-test-recording-long` is available for duration-bounded disk-throughput runs |

The strict real-device matrix was also run on the attached Apple Silicon Mac
with `AURA_REAL_DEVICE_STRICT=1`: software device transition, CoreAudio,
VST3, AU and CLAP all returned `PASS` with zero `SKIPPED` capabilities. The
latest local matrix report was
`/tmp/aura-real-device-evidence-latest/matrix-20260827-064855.tsv`;
the timestamped TSV and per-capability logs are emitted under the configured
evidence directory for each run.

This is machine-specific historical evidence, not a universal compatibility claim. The
following remain explicitly **not evidenced** by this run: Vital CLAP state
restore (the installed CLAP instrument fixture currently times out while the
audio sequence completes and is isolated as a failed capability rather than
treated as pass),
Vital's full
parameter/audio equivalence after a complete project save and application
restart (the current test proves finite non-silent note audio, not waveform
identity), OpenUtau's actual audio export from the OpenUtau application and
re-import of that newly exported file inside the DAW (the current evidence
covers the real OpenUtau project UI plus Aura's validated import/persistence
handoff using an existing WAV),
multi-hour recording/autosave, sleep/wake, physical MIDI/SysEx, and every AU
component or architecture combination.

The recording contract proves bounded lifecycle behavior, exact stop-time
draining, and RF64-capable publication, but it does not substitute for a
multi-hour disk-throughput run. A 60-second local recording stress run and a
separate 5-minute recording stress run completed successfully with
finalized-frame and temporary-file checks. A 5-minute time-budgeted worker
lifecycle run also completed 87 rounds without exceeding its resource budgets.
The new `aura-test-recording-long` target
exercises configurable duration and block volume and fails on dropped frames,
writer errors, payload-size mismatch, or temporary-file residue. A multi-hour
hardware run must still record device, filesystem, duration, dropped frames,
writer errors and final WAV metadata before being described as long-running
hardware evidence.

The realtime callback soak is independently available as the ignored
`callback_soak_has_no_nonfinite_output_or_deadline_misses` integration test.
It runs for `AURA_REALTIME_SOAK_SECONDS` (30 seconds by default), exercises
control-plane changes while processing 256-frame blocks, and fails on any
deadline miss or non-finite sample. A one-second smoke run passed locally;
longer runs remain software-host evidence until repeated on physical devices.

CI invokes the workspace suite with `--test-threads=1`; this is required by
the native engine's process-wide FFI lifecycle and prevents concurrent test
teardown from invalidating another test's engine instance.

The real-device matrix bounds AU and CLAP probes to 90 seconds. A third-party
component that does not return within that window is recorded as `FAIL` with
its captured log; it is never converted into a release pass by an indefinite
wait or a silent skip.

The current local bundle passes `codesign --verify --deep --strict` and has a
valid designated requirement. `spctl --assess` remains rejected until a
Developer ID signature is notarized; local ad-hoc/identity signing is not
reported as distribution-ready notarization.

The freshly built application and plugin worker report Mach-O `arm64` and are
not fat/Universal binaries. Intel execution and Universal packaging therefore
remain explicitly unverified until an x86_64 build host or cross-target build
is exercised.

## Evidence artifacts for real plugins

Every real-plugin validation run should record:

- plugin format, bundle identifier, version and architecture;
- worker capability JSON and selected worker binary;
- project generation and audio configuration generation;
- pre/post parameter dump and state checksum;
- render hash or bounded audio comparison result;
- worker restart/quarantine result;
- dropout, overrun, NaN/Inf, RSS and FD counters;
- device, sample rate and block size;
- exact commit and clean-checkout status.

Without those artifacts, the correct status is **not evidenced**, not
**passed**. This distinction is intentional: a fork can add a new plugin or
platform without inheriting a misleading global success claim.

### Current hardware boundary (2026-08-29)

`AURA_REQUIRE_HARDWARE_DEVICE=1 scripts/run_device_transition_contract.sh`
failed closed because this host had no usable CoreAudio output device. The
software fallback and 20-cycle reconfiguration matrix passed, but physical
device start/stop/reconfigure remains **not evidenced** here.

### Current workspace regression (2026-08-29)

The current checkout passes `cargo check --workspace --locked` and
`cargo test --workspace --all-targets --locked -- --test-threads=1`. The full
workspace run completed with zero failures, including 468 core unit tests,
33 UI unit tests, and 4 UI/Core integration tests. This is local software
evidence; it does not upgrade hardware- or third-party-dependent checks.

The current UI package also passes its 33 unit tests and 4 UI/Core integration
tests with `cargo test -p aura-ui --locked --no-default-features --tests`.
These tests cover the production workflow, transport callbacks, template
navigation, render output handoff, and empty-peak handling; they do not replace
manual GUI and hardware acceptance.

OpenUtau round-trip evidence is now reproducible with
`scripts/run_openutau_roundtrip.sh`. It requires an explicit renderer command
and records source/output hashes plus `ffprobe` audio metadata; without those
external inputs it reports `SKIPPED` (or fails in strict mode).

## Recommended public wording

> Aura is an MIT-licensed, programmable macOS DAW engine and early technical
> preview. CLAP and macOS AU are exercised through an isolated worker. VST3,
> Surge XT and OpenUtau import handoff are validated by capability-specific
> fixtures and evidence rather than bundled binaries. Vital remains an
> optional host target and requires a separately installed, tested fixture.
