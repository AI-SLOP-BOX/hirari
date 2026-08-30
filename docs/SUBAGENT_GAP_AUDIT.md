# Aura DAW サブエージェント横断ギャップ監査

更新日: 2026-08-12

6系統の読み取り専用監査（Rust DSP、C++/FFI、Slint/UI、プレビュー機能、ビルド構成、DSP境界）を統合した結果。既存の修正済み項目は重複計上していない。

## 結論

`cargo check/test --workspace` が成功しても、プレビューの実行可能性を保証していない。現在の最重要課題は次の順序である。

1. **実際にビルドされるソースと、検証対象を固定する**
2. **異常入力で落ちないAudio/DSP境界を閉じる**
3. **UIだけで完了したように見える操作を、実装済みか未対応表示に整理する**
4. **非macOS音声経路とGPUフォールバックを明示する**
5. **no-opの大機能は公開UIから隔離する**

## P0 — プレビュー前に止血必須

### 1. workspace外のRustコードが検証されていない

- `aura-audio-engine/src/` は `Cargo.toml` がなく、workspaceのcheck/test対象外。
- `elastic_warp.rs` は `output.len() > input.len()` で `copy_from_slice` がpanicし、warp/pitch-shift本体もno-op。
- `elastic_audio.rs` は異常ratio時の負のオフセットが巨大なusize添字へ変換され得る。
- 旧経路の `aura-audio-engine/src/drum_machine.rs` には固定ダミー音源が残る。

対応方針: 正式crate化してテスト対象へ入れるか、参考コードとして明示的に隔離する。公開APIに残すなら、no-opではなく`Result`で未実装を返す。

### 2. 入力レート・タイムストレッチの境界が未閉鎖

- `src/dsp/analysis/audio_resampler.hpp` はsource/target rateの0、負数、NaN、Infを未検証。
- `src/dsp/analysis/time_stretcher.hpp` はNaN ratio、負の探索位置、ブロック参照範囲の境界が未保証。
- `aura-core-bridge/src/forensic.rs` は`channels == 0`、空入力で除算経路が残る。

対応方針: 有限値・正値チェック、最大出力長、checked計算、範囲外のゼロパディングまたは明示エラーを共通ヘルパー化する。

### 3. 非macOSのAudioDeviceが実音声へ到達しない

- `src/core/audio_engine.cpp` は`MacAudioDriverHost`を直接生成。
- 非macOSの`createAudioDevice()`は`SilentAudioDevice`を返す。
- 旧`audio_driver_factory`系もAPI指定を無視してSilent/Dummyへ寄る経路がある。

対応方針: `AudioDevice`を唯一の抽象化にし、Windows/Linuxは実装または明示的な非対応状態にする。Silent fallbackを「再生可能」と表示しない。

## P1 — プレビュー品質を損なう重大な穴

### 4. C++/Vulkan/プラグインが「初期化成功に見える未実装」

- `src/graphics/platform/vulkan_kernel.cpp` はsurface、swapchain、command buffer、pipelineを使わず`true`を返す。
- `vulkan_kernel.hpp` はVulkan型を使うがヘッダー自身のincludeが不足し、独立コンパイルできない可能性がある。
- `cleanup()`の定義不足、失敗経路のinstance解放漏れがある。
- `PluginHost`、VST3/CLAP hostは検出・シンボル確認段階で、instance/process/reset/stateが未接続。

対応方針: 完全実装まではVulkan/Pluginを成功扱いしない。Metal/CPUへ確実にfallbackし、プラグインは「未対応」と表示する。

### 5. RTスレッドとプロセス境界の安全性

- `mac_audio_driver_host.mm` のdriver lifecycleがmutexなしでstart/stop/reconnectされ、並行操作でUAFの危険。
- `distributed_plugin_host.hpp` の共有バスは共有メモリではなく、非atomicデータをseqlock風に読書き。
- RT処理中に`std::function` fallbackを呼ぶ。

対応方針: lifecycleを停止完了後に破棄する状態機械へ変更。音声データは固定二重バッファまたはIPC SPSC、callbackは固定関数ポインタへ限定する。

### 6. UI操作の未接続と誤認表示

- `aura-ui/src/slint_ui.rs` の録音は入力→録音バッファ→WAV→リージョン生成まで接続済み。FX bypassなど別操作は未接続箇所が残る。
- `toggle_fx` は実DSPチェーンのbypass APIへ接続済み。対象プロセッサが存在しないFXはUIを更新せず未接続表示。
- `aura_studio.slint` のFILE/EDIT/TRACK、PREFS、GENERATE PATTERN、AUTO ARRANGE等に空TouchAreaが残る。
- Open/Saveはboolに潰され、破損・権限・欠落を区別できない。

対応方針: プレビュー対象外はdisabled＋「未対応」を明示し、対象にする操作だけcallback→engine→結果表示まで接続する。エラーはcode/message/recoverableを返す。

### 7. UIの全件生成と更新過多

- `aura_studio.slint` は全トラック、全クリップ、全ノート、CC、automation点を常時ノード化。
- `aura-ui/src/slint_ui.rs` は約16ms周期でCPU、デバイス、再生位置、FFT、スペクトル、動画、ラウドネスをまとめて更新。
- 約128ms周期でも全トラック・全クリップを走査して波形モデルを置換。
- 再生ヘッドがトラック単位・概要単位で重複生成される。

対応方針: 再生ヘッド、メーター、解析、波形キャッシュの更新周期を分離。可視範囲＋overscanのvirtualization、dirty clip更新、再生ヘッド1本のoverlayを導入する。

## P2 — 公開前後に整理する設計負債

### 8. no-op大機能が公開APIに残る

新規監査で次の空実装が確認された。

- `offline.rs` / `export.rs` / `bounce_core.rs`: オフラインレンダー・書き出し
- `pdc_graph.rs` / `pdc_manager.rs`: PDC solver・再計算
- `param_tree.rs`: パラメータ登録
- `midi_mapping_manager.rs`: MIDI CC処理
- `bus_router.rs` / `bus_track.rs`: バス再構築・音声合算
- `live_loops_engine.rs`: セル発火
- `resources.rs`: ライブラリ統合
- `auditor.rs`: routing anomaly検出

対応方針: ①プレビュー必須なら実装、②未対応なら`Unsupported/Incomplete`を返す、③UIから隠す、のいずれかを機能ごとに決める。常時`true`のauditは正常判定に使わない。

### 9. 監査と診断が壊れた状態を正常扱いする

`audit_*`が無条件`true`を返す箇所が多数残り、`diagnostics.rs`にも固定timestamp、dropouts=0、memory=512MBのダミー値がある。これは保守性ではなく、障害の隠蔽につながる。

対応方針: `AuditReport`に対象、検証時刻、失敗理由、未実装状態を持たせる。診断値は実測できない場合に`Unknown`を返す。

### 10. ビルド・テストの再現性不足

- C++/Objective-C++の多数のtranslation unitがCargo build対象外。
- Vulkan、Metal、SCAE、DSPの一部cpp、旧UIは実ビルドに含まれない。
- CI設定がなく、UI smoke、C++ callback、CoreAudio切断、Open/Save/Bounce実行が未検証。
- `Cargo.lock`が除外されている。
- `build.rs`のheader依存追跡が不完全。

対応方針: `cargo fmt/check/clippy/test/build`、OS別native build、C++ syntax/link、UI smoke、仮想AudioDeviceテストをCI化する。アプリworkspaceとして`Cargo.lock`を追跡する。

## 追加DSP候補

P0/P1の止血後に以下を処理する。

- `psychoacoustic_model.hpp`: `log(0)`・負周波数
- `granular_cloud.hpp`: 0長bufferの`%`、異常sample rateによる巨大確保
- `linear_phase_eq.hpp`: 2ch固定overlapへ3ch以上を渡す経路
- `phase_vocoder.hpp`: FFTサイズシフトのoverflow、衝突binの上書き
- `stereo_tremolo.rs`: 極端BPMによるInf位相

## 実装順

1. `aura-audio-engine`の扱いを決め、elastic/forensicのpanicを修正
2. AudioDevice factoryを一本化し、非macOSを正直な状態表示にする
3. UIの空TouchArea・録音・FX bypass・Open/Save errorを整理
4. Vulkan/Pluginは成功扱いを止め、fallback/未対応表示を実装
5. UI更新分離とvirtualizationを導入
6. no-op機能を実装・隔離・削除の3分類へ整理
7. CIとUI/C++/Audio integration smoke testを追加

## 検証状況

今回のRustワークスペーステストは98件成功しているが、本監査で列挙した未参加経路は含まれない。したがって「98件pass」はプレビュー完成の判定ではなく、Rust既存経路の回帰確認に限定する。

## 今回の実装済み項目

- `elastic_warp_engine.rs` の実状態監査と探索範囲保護
- `elastic_audio.rs` のratio、短バッファ、符号付き探索位置の保護
- `forensic.rs` の空入力・0チャンネル・NaN保護とステレオ相関
- `audio_resampler.hpp` の無効レート拒否、出力上限、補間境界保護
- `time_stretcher.hpp` のratio・ポインタ・WSOLA範囲保護、`rand()`除去
- `automation_mode.rs` のLatch状態保持とモード遷移リセット
- `classification.rs` の空入力・非有限値・無効sample rate処理
- `pitch_detector.rs` のsample rate、NaN、MIDI範囲保護
- `resources.rs` の音声ファイル走査、最短候補解決、プロジェクト統合コピー
- `diagnostics.rs` の実時刻記録、イベント上限、CPU値の上限処理

上記実装後も、実オーディオグラフ接続、PDCの実グラフ統合、Vulkan実デバイス描画、プラグインDSP/UI接続、UI仮想スクロールは未完了である。

## PDC・ルーティング実装の追加

- `pdc_graph.rs`: Kahn型トポロジー計算、依存遅延の最大値伝播、cycle/未知ノード監査
- `pdc_manager.rs`: トラック・バスの最大遅延に対する補正オフセット計算
- `bus_router.rs`: ルーティングの実行レベル再構築と重複ノード監査
- `bus_track.rs`: 有限値・位相反転・gain適用
- `param_tree.rs`: パラメータ登録、範囲クランプ、実値取得、監査
- `midi_mapping_manager.rs`: MIDI Learn時のchannel/CCキー登録と入力検証

これらは公開APIの形を維持した最小実装であり、実際のオーディオグラフとの接続、MIDI値のパラメータ反映、Bus入力の合算は次段階で実装する。

## Live Loop・書き出しキューの追加実装

- `live_loops_engine.rs`: セルのBar/Beat/16分/即時量子化、同一行の排他再生、queued→playing遷移を実装
- `offline.rs`: 空ジョブ・空ステップを拒否し、renderer未接続を`last_error`で可視化
- `export.rs`: 不正な書き出しジョブを拒否し、renderer未接続を成功扱いしない
- `bounce_core.rs`: タスク妥当性とrenderer未接続状態を監査可能化

`export.rs` に有限値・フレーム整合性を検証するPCM16 WAV writerとatomic rename経路を追加した。`offline.rs` / `bounce_core.rs` からも実バッファを渡せるが、renderer未接続時は成功にしない。実際のオーディオグラフ／各種エンコーダ接続は未完了である。

## 録音・Bounce preview経路の追加

- `recording_preview.rs`: 事前確保済みinterleaved f32バッファ、NaN/Inf・チャンネル不整合・容量超過の拒否、非破壊region snapshot、再利用可能なstop/startを実装
- `export.rs`: 完成済みinterleaved bufferからPCM16 WAVへatomic書き出しを実装
- `offline.rs` / `bounce_core.rs`: 実バッファ書き出しAPIを追加し、空データ・不正タスク・renderer未接続を明示的に失敗させる
- 追加テスト7件を含め、recording/bounceのpreview経路を回帰検証

## 追加回帰テスト

- Live Loopの量子化境界・同一行排他再生
- PDCグラフのcycle検出・下流遅延伝播
- PDC managerのtrack/busオフセット再計算

これらを含め、workspaceテストは98件全件成功。

## 追加の一括実装

- `transport.rs` / `transport_manager.rs`: サンプル位置のsaturating加算、無効BPM/SR拒否、cycle範囲検証、録音状態の整合化
- `automation_recorder.rs`: flush後の間引き結果保持と状態監査
- `zero_crossing_engine.rs`: target位置のクランプ、NaNサンプルの安全化
- `automation_curve.rs`: NaN/Inf点拒否、重複時間の安全化、指数補間、実状態監査
- `aura_studio.slint`: 接続されていないプレビュー操作を「未対応」表示へ変更し、空TouchAreaを整理
- `audio_device.hpp` / driver factory: Silent fallbackをハードウェア未使用・起動失敗として明示
- `vulkan_kernel.hpp/cpp`: Vulkan無効・未接続状態を成功扱いせず、cleanupとfallback状態を明示

検証結果は`cargo check --workspace`成功、`cargo build --workspace`成功、workspaceテスト98件成功。UIバイナリは起動スモークでSlint初期化到達を確認。Vulkan有効ビルドと実デバイス接続は別途環境依存の検証が必要。

## プレビュー操作・Host状態の追加実装

- `aura-ui/src/slint_ui.rs`: 再生ヘッド16ms、解析64ms、波形・低頻度テレメトリ128msへ更新周期を分離
- `plugin_host.hpp` / VST3 / CLAP host: library検出、instance生成、process接続を状態分離
- 外部Plugin状態を`Unloaded` / `LibraryResolved` / `NotInstantiated` / `Unsupported` / `Failed` / `Operational`として明示
- 未接続processを成功扱いせず、RT側ではatomic状態参照に限定

検証結果は`cargo check --workspace`成功、workspaceテスト98件成功。実プラグインのDSP/UI処理そのものは、SDK接続とサンドボックス実装が別途必要。

## 追加の診断・MIDI実装

- `midi_mapping_manager.rs`: MIDI Learnでマッピングと最新CC値を保持し、孤立値を監査
- `auditor.rs`: 8-byte edge payloadの整列検証、自己ループ・重複・cycle検出
- `recording_session.rs`: UI表示と録音バッファのライフサイクルを結び、Idle/Recording/Stoppedを明示
- `slint_ui.rs`: 録音ボタンをRecordingSessionへ接続し、開始・停止・入力エラーをUIへ反映。タイマーからCoreAudio入力キューを非RTポーリングし、停止時は選択トラックへリージョン登録
- `AuraCore`: RecordingSessionのstart/append/stop APIを公開し、UI表示と実録音状態を分離
- `vulkan_kernel.hpp/cpp`: Vulkan型の条件付きinclude、初期化失敗時cleanup、未完成描画の明示的失敗
- `mac_audio_driver_host.mm/.hpp`: start/stop/reconnectのライフサイクル直列化とcallback寿命の分離
- `psychoacoustic_model.hpp` / `stereo_tremolo.rs`: 無効周波数・異常BPM・非有限値の安全化
- `granular_cloud.hpp` / `linear_phase_eq.hpp` / `phase_vocoder.hpp`: 0長、チャンネル数、FFTサイズ、overflowの防御
- `ffi_implementation.cpp` / `analysis_hub.cpp`: イベントlabel終端、所有スナップショット、GPU状態通知を追加
- `audio_engine.hpp/.cpp` / `build.rs`: 非macOSでMacAudioDriverHostへリンクしない明示的なUnavailable経路を追加
- `.github/workflows/ci.yml`: Ubuntu/macOSのcargo check/testとmacOS native toolchain検証を追加
- `mac_audio_driver.hpp/.mm`: 事前確保済みCoreAudio入力を受けるatomic sink登録APIを追加。callbackからRustを直接呼ばず、未登録時は入力を破棄
- `mac_audio_driver_host.hpp/.mm`: 8ブロック×2ch×4096フレームの固定容量SPSC入力キューとpoll APIを追加。満杯・不正入力はdrop数として通知
- `audio_engine.hpp/.cpp` / `lib.rs`: CoreAudio入力キューを非RT側からpollし、interleaved Rust bufferとしてRecordingSessionへ渡すAPIを追加。UI録音経路から定期利用
- `aura_unified_engine.cpp` / `audio_engine.cpp`: DSPブロック実測時間とブロック予定時間からCPU負荷を算出し、atomic telemetryとしてUIへ公開。未計測値を固定値で埋めない
- `lib.rs`: 停止済みプレビューをプロジェクト隣の`Audio Recordings`（未保存時はOS一時領域）へアトミックWAV書き出しし、既存のnative decoder/import経路へ渡すcommit APIを追加
- `analysis_hub.hpp/.cpp`: C++内部の解析結果を`std::vector`で保持し、CXX境界でのみ`rust::Vec`へ変換。UI完全リンク時の`PlainClash`未定義シンボルを解消
- `effect_chain.hpp` / `track.hpp` / `aura_unified_engine.*` / `audio_engine.hpp` / `lib.rs` / `slint_ui.rs`: FX bypassをUIから実DSPチェーンへ接続し、未接続状態を成功扱いしない
- `aura_unified_engine.*` / `audio_engine.hpp` / `lib.rs` / `slint_ui.rs` / `aura_studio.slint`: 内部実装済みのAura Limiterをコマンドパレットから選択中トラックへ挿入可能化。挿入失敗時はUI状態を更新しない
- `track.hpp` / `aura_unified_engine.cpp`: 内部プラグイン種別を`TrackState.pluginData`へ保存し、プロジェクト読込時に検証済みの種別だけを再構築。外部プラグインを存在するように偽装しない
- `aura-core-bridge`: `cargo clippy --fix`で安全な機械的警告（Default実装、`map_or`、`is_multiple_of`、不要な変換など）を一括整理。DSPのインデックスループは性能と可読性を確認し、無理な自動変換を避けた
- `aura-core-bridge` / `aura-ui`: Clippy残存警告を整理し、`cargo clippy --workspace --all-targets -- -D warnings`を成功化。UI専用共有は`Arc`から`Rc`へ変更し、非`Send`のAudioCoreを誤ってスレッド共有しないようにした
- `cargo fmt --all`、`cargo build --workspace`、UIバイナリ起動スモークを実施。Slint初期化メッセージ到達後に手動停止し、起動時クラッシュがないことを確認
- `audio_engine.hpp` / `lib.rs` / `slint_ui.rs`: `try_set_playing`を追加し、実音声デバイス未起動時の再生開始を拒否。無音状態を再生中と誤表示しない
- `slint_ui.rs`: テストトーンも音声デバイス状態を確認してから開始し、未接続時の成功表示と無音セルフテストを防止
- `slint_ui.rs`: Open/Saveのファイル選択キャンセルを`CANCELLED`として表示し、失敗とユーザーキャンセルを混同しないよう整理
- `aura_unified_engine.cpp`: プロジェクト内の相対音声パスをプロジェクトファイルの親ディレクトリ基準で解決し、移動・再オープン後も音声リージョンを復元
- `plugin_host.hpp` / AU/VST3/CLAP host: 実process関数がない状態をOperationalにしない状態契約と診断を追加
- 追加回帰テスト14件を含め、workspaceテストは98件全件成功
