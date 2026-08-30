# Empty / Stub Implementation Audit

更新日: 2026-08-09

## 概要

サブエージェント3体で、C++ core/DSP、Rust bridge、UI/FFI/buildを読み取り専用調査した結果。

通常の境界値処理（空キュー、未登録ID、エラー時の `nullptr` など）は除外した。すでに実装済みの項目も除外し、現在なお「空実装」「固定値」「入力無視」「未接続」の可能性が高いものを記載する。

## 優先度の定義

- P0: ユーザー操作とエンジン状態が破綻する、またはデータを失う
- P1: 主要機能が動作しない、または正常動作に見せかける
- P2: 補助機能・解析・再現性・テストの問題
- P3: 品質改善・将来の不具合予防

## 最優先対応

### P0 — UIとエンジンの状態不一致

- [slint_ui.rs:132-139](../aura-ui/src/slint_ui.rs:132)
  - `delete_track` がUIモデルだけを削除し、`core.remove_track()`を呼ばない。
- [slint_ui.rs:224-233](../aura-ui/src/slint_ui.rs:224)
  - `toggle_cloner` がUI状態だけを変更し、エンジン呼び出しがコメントアウトされている。
- [slint_ui.rs:214-216](../aura-ui/src/slint_ui.rs:214)
  - UI clip IDをそのままengine region IDとして渡している。

推奨: UIモデルのIDとengine IDを分離し、Core操作の成功後にUIを更新する。失敗時のrollbackも追加する。

## C++ Core / DSP

### P1

- [session_io.hpp:39-48](../src/core/io/session_io.hpp:39)
  - `loadProject()`がファイルを開くだけで、パース・復元をしない。
- [audio_decoder.hpp:149-168](../src/core/io/audio_decoder.hpp:149)
  - MP3分岐が常に`nullptr`。実デコーダ未接続。
- [cinematic_suite.hpp:41-59](../src/dsp/effects/cinematic_suite.hpp:41)
  - DivineReverb、MasterLimitPro、AnalogClonerが入力を処理しない。
- [analyzer_8khz.cpp:17-27](../src/dsp/analysis/analyzer_8khz.cpp:17)
  - `analyze()`が空、`getEnergy()`が常に0。
- [pitch_detector.hpp:20-27](../src/dsp/analysis/pitch_detector.hpp:20)
  - 入力を見ず常に0Hz。
- [spectral_matcher.hpp:35-49](../src/dsp/analysis/spectral_matcher.hpp:35)
  - 平均更新・ピンクノイズ参照・マッチカーブが未実装。
- [stem_splitter.hpp:27-35](../src/dsp/analysis/stem_splitter.hpp:27)
  - `split()`が常に空のStem結果。
- [va_oscillator.hpp:34-41](../src/dsp/synthesis/va_oscillator.hpp:34)
  - 仮想アナログオシレータが常に無音。
- [lfo.hpp:27-36](../src/dsp/synthesis/lfo.hpp:27)
  - 波形・位相を処理せず常に0。

### P2

- [iprocessor.hpp:56-73](../src/dsp/iprocessor.hpp:56)
  - 状態・パラメータ・レイテンシ等のデフォルト値が未接続を隠す。`isParameterAutomated()`も常にtrue。
- [celestial_reverb.hpp:29-35](../src/dsp/plugins/celestial_reverb.hpp:29)
  - パラメータ取得が0.5固定、設定値を無視。
- [mastering_kernel.hpp:48-55](../src/core/mixing/mastering_kernel.hpp:48)
  - LUFSが-14固定、write pointerがnullptr。
- [track.hpp:173-178](../src/core/engine/track.hpp:173)
  - Trackのspectrogram APIが空配列。
- [automation_controller.hpp:22-28](../src/core/engine/automation_controller.hpp:22)
  - Touch/Latch状態を処理せず常にcurve値を返す。
- `advanced_audio_dsp_*` 系のmasking計算が常に0。

### P3

- [neural_bridge.hpp:33](../src/core/neural_bridge.hpp:33)
  - パケットtimestampが常に0。
- [analysis_hub.cpp:137](../src/core/analysis_hub.cpp:137)
  - `trigger_background_analysis()`が空。

## Rust bridge / background systems

調査対象には、実処理を行わず成功扱いする公開APIが複数残っている。

### P1

- [unified_engine.rs:18-30](../aura-core-bridge/src/unified_engine.rs:18)
  - 統合renderが各サブシステムを接続していない。
- [professional_suite.rs:78-98](../aura-core-bridge/src/professional_suite.rs:78)
  - Dynamic EQ / Tape Saturationが入力を変更しない。
- [loudness.rs:24-29](../aura-core-bridge/src/loudness.rs:24)
  - Loudness処理が入力を無視し、初期値のまま。
- [voice_manager.rs:68-78](../aura-core-bridge/src/voice_manager.rs:68)
  - Voice renderが出力バッファへ書き込まない。
- [sequencer.rs:22-28](../aura-core-bridge/src/sequencer.rs:22)
  - MIDIイベントを生成しない。
- [bus_system.rs:20-30](../aura-core-bridge/src/bus_system.rs:20)
  - bus加算・processが状態を変更しない。
- [pdc.rs:15-25](../aura-core-bridge/src/pdc.rs:15)
  - PDCが全て0。
- [resource_manager.rs:21-36](../aura-core-bridge/src/resource_manager.rs:21)
  - asset scan / recovery / consolidationが空。
- [advanced_export_engine.rs:33-38](../aura-core-bridge/src/advanced_export_engine.rs:33)
  - Export jobを実行しない。
- [track_freeze_manager.rs:12-22](../aura-core-bridge/src/track_freeze_manager.rs:12)
  - Freeze状態を変更しない。

### P2

- [smart_controls_manager.rs:30-35](../aura-core-bridge/src/smart_controls_manager.rs:30)
  - mappingを参照せず値を適用しない。
- [sidechain_manager.rs:29-34](../aura-core-bridge/src/sidechain_manager.rs:29)
  - link解決を行わない。
- [marker_system.rs:49-54](../aura-core-bridge/src/marker_system.rs:49)
  - tempo同期位置が更新されない。
- [mixer_telemetry.rs:33-37](../aura-core-bridge/src/mixer_telemetry.rs:33)
  - meter値が更新されない。
- [auto_save_manager.rs:27-32](../aura-core-bridge/src/auto_save_manager.rs:27)
  - startしても保存処理が動かない。
- [notification_system.rs:27-38](../aura-core-bridge/src/notification_system.rs:27)
  - event内容を破棄する。
- [smart_file_classifier.rs:20-30](../aura-core-bridge/src/smart_file_classifier.rs:20)
  - Unknown / 0 / 120 BPM / Cを固定返却。
- [selection_based_processor.rs:10-17](../aura-core-bridge/src/selection_based_processor.rs:10)
  - offline処理入力を破棄。
- [shared_memory_ipc.rs:22-27](../aura-core-bridge/src/shared_memory_ipc.rs:22)
  - IPCフレーム本体を破棄。

追加のRust監査項目：

- [vintage_eq.rs:79](../aura-core-bridge/src/vintage_eq.rs:79)
  - 係数が単位ゲイン固定で、パラメータ変更が音に反映されない。
- [mastering.rs:72](../aura-core-bridge/src/mastering.rs:72)
  - DDP exportは入力検証のみで、実DDPパッケージ生成は未接続。成功を偽装しない。
- [persistence.rs:47](../aura-core-bridge/src/persistence.rs:47)
  - バックアップtimestampが固定値。
- [persistence.rs:73](../aura-core-bridge/src/persistence.rs:73)
  - checksumがデータ内容ではなく長さのみ。
- [lib.rs:401](../aura-core-bridge/src/lib.rs:401)
  - FFI公開メソッドに`unwrap()`が多く、初期化失敗・破棄後アクセスでpanicする。
- [cinematic_suite.rs:25](../aura-core-bridge/src/cinematic_suite.rs:25)
  - サイズ0のAllPassFilterでゼロ除算/範囲外アクセスの可能性。
- [routing_graph_pdc.rs:43](../aura-core-bridge/src/routing_graph_pdc.rs:43)
  - 未登録ノードを指定するとunwrapでpanicする。
- [templates.rs:41](../aura-core-bridge/src/templates.rs:41)
  - テンプレート名を破棄し、検索・生成しない。
- [master_suite.rs:51](../aura-core-bridge/src/master_suite.rs:51)
  - 複数の`audit_*` APIが状態を検査せず無条件にtrue。
- [video.rs:64](../aura-core-bridge/src/video.rs:64)
  - FPSの0・NaN・負値を拒否しない。

未接続の代表的なRust API：

- `AuraUnifiedOrchestrator::render_block`
- `BusSystemOrchestrator::add_samples/process`
- `BounceCoreOrchestrator::execute_professional_batch_render`
- `TemplateOrchestrator::instantiate_template`
- `PdcOrchestrator::recalculate_pdc`
- `MasteringOrchestrator::export_ddp`
- `ResourceOrchestrator::scan_library/resolve_missing_assets/consolidate_project`

## Plugin / UI / FFI

### P1

- [aura_studio.slint:67-131](../aura-ui/ui/aura_studio.slint:67)
  - set_bpm、rename_clip、duplicate_track、toggle_record、set_filter、set_route、automation_point_moved、add_note、delete_note、quantize_notes等のcallbackが未接続。
- [slint_ui.rs:374-385](../aura-ui/src/slint_ui.rs:374)
  - latency表示が0固定。
- [slint_ui.rs:22-26](../aura-ui/src/slint_ui.rs:22)
  - 波形を乱数生成。
- [slint_ui.rs:48-85](../aura-ui/src/slint_ui.rs:48)
  - Track / marker / sample catalogが固定データ。
- [slint_ui.rs:110-112](../aura-ui/src/slint_ui.rs:110)
  - Track type変換がマジックナンバー。
- [lib.rs:230-241](../aura-core-bridge/src/lib.rs:230)
  - FFI event labelの固定128byte契約が脆い。

### P2

- [build.rs:18-25](../aura-core-bridge/build.rs:18)
  - globによるCPP自動収集が環境依存。
- [build.rs:31-50](../aura-core-bridge/build.rs:31)
  - 最適化フラグとCPU featureが常時/環境変数依存。
- [aura-ui/build.rs:1-3](../aura-ui/build.rs:1)
  - Slint入力のrerun-if-changedがない。
- [lib.rs:624-749](../aura-core-bridge/src/lib.rs:624)
  - テストがDSP smoke testに集中し、UI/FFI/ID/単位を検証しない。
- [lib.rs:727-748](../aura-core-bridge/src/lib.rs:727)
  - Bounceテストが戻り値と一意な出力ファイルを検証しない。

## Build / documentation / licensing

### P0

- `aura-core-bridge` / `aura-ui` のGit管理・submodule構成がクリーンcloneで再現できない可能性がある。

### P1/P2

- CMake入口とREADMEの説明が実際のCargo/Slint構成と不一致。
- README/L本体のGPL依存説明とCargo依存が一致しない。
- `THIRD_PARTY_NOTICES.md`削除によりライセンス参照が壊れている可能性。
- READMEはGPUIを説明するが、実装はSlint。
- Cargo lock/version固定と`--locked` CIが不足。

## 実装済みとして扱った項目

今回の監査では、過去の修正で実装済みになった以下は未対応リストから除外した。

- ProjectSerializerの保存/読み込み
- Transport状態
- Automation Curve / Track Automation
- Track/Region再生とBounce長
- UI track ID / scrubのBPM・sample rate変換
- TakeManager / Spatial Mode
- ProcessGraph / BusRouter
- MIDI blacklist / rate limiter
- AnalysisHubのphase、motion、spectral partials、mel、song structure
- EngineStats、Video frame cache、Vibe scores
- Patch index / sample cache

## 推奨ロードマップ

1. UIとengineの削除・clip ID・Cloner同期を修正
2. C++主要DSP（LFO、VA oscillator、pitch、spectral matcher）を実装または未対応表示へ変更
3. RustのExport、Resource、Voice、Sequencer、PDCを接続
4. callback網羅チェックとFFI統合テストを追加
5. Build/submodule/LICENSE/READMEをクリーンclone可能な構成へ整理
