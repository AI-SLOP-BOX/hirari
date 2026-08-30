# Aura DAW 深掘り未実装・未接続監査

調査日: 2026-08-10

最新の横断監査と未実装一覧は[`UNIMPLEMENTED_GAPS.md`](../docs/UNIMPLEMENTED_GAPS.md)に統合した。こちらは実装履歴と詳細根拠を残す。

3つのサブエージェントによる読み取り専用調査と、メインエージェントの横断検索を統合した一覧。ここでいう「実装済み」は、コードが存在するだけでなく、UIまたは実オーディオ経路から呼ばれることを基準にする。

## 2026-08-10 実装反映

- UI初期モデルはエンジン状態が空なら空モデルへ同期するよう修正。
- Open時に保存済みサンプルレートでDSPを再準備するよう修正。
- AuraCore初期化失敗時の`expect`終了をUI上のエラー表示へ変更。
- 波形UIは既存の周期ポーリングで生成完了後に再取得し、同一リージョンの重複解析ジョブを抑制。プロジェクト切替・音源差し替え時の古い解析結果を世代番号で破棄するよう修正。
- `pitch.rs`の周波数0／非有限値ガード、比率クランプ、ブロック監査と回帰テストを追加。
- `graph_solver.rs`を公開モジュール化し、循環依存・未知依存を`Result`で返し、キャッシュ重み順を実装。回帰テストを追加。
- Envelopeモジュレーターにノード状態、Attack／Decay／Sustain／Release、ゲート処理を追加。回帰テストを追加。
- `vocal.rs`に有限値サニタイズ付きの軽量デエッサー／ゲイン制御、`holographic.rs`に距離減衰・等パワーパンを追加。各回帰テストを追加。
- WAV統合テストの一時ファイル名を一意化し、並列テストのファイル競合を解消。
- WAVデコーダの実行経路から`unwrap()`を除去。切断チャンク、短いfmt/data領域、途中で切れたサンプルはパニックせず`Result`エラーとして返すよう修正。
- `sampler.rs`のClassic／Granular／Additive／Spectralを実サンプル入力ベースの処理へ置換。空入力の無音化、有限値サニタイズ、不正ゾーン監査と回帰テストを追加。
- `unified_engine.rs::render_block`を入力検証付きのRust側統括境界へ変更。有効ブロックのサンプル数・サンプルレートを原子カウンタへ記録し、実オーディオ処理は現行C++グラフへ二重実行せず委譲する構成を明示。
- `hardware.rs`のMCU／HUI／EuCon／OSCイベント経路を共通の正規化処理へ接続。未知コントローラ、NaN／Inf、範囲外値を拒否し、回帰テストを追加。
- `export.rs`に検証付きPCM16 WAV書き出しを追加。テンポラリファイル、`sync_all`、アトミック置換、非有限値／フレーム不整合の拒否を実装し、回帰テストを追加。
- `selection_based_processor.rs`に選択範囲のPCM16 WAV書き出しAPIを追加。範囲検証後に`export.rs`を呼び、成功したデータだけを保留キューへ登録する回帰テストを追加。
- C++の`add_plugin`を完全なno-opからトラックのプラグインスロット登録へ変更。未知タイプは拒否し、外部VST／AU／CLAPの実インスタンス生成・音声処理は未接続のまま明示。
- UIのサンプルブラウザから固定のダミー音源を除去。実際にプレビューしたファイルを名前・形式・サイズ付きでカタログへ追加し、検索対象へ反映するよう修正。
- UIの固定レイテンシー／バッファ表示を削除。C++エンジンのブロックサイズとサンプルレートから実測可能なブロック遅延をRust／Slintへ公開するよう修正。
- UIの録音・トラックアーム・FXトグルcallbackを接続。モデル状態は更新し、エンジン未接続の録音入力／FXプロセッサはアクション履歴へ明示するよう修正。
- `prepareToPlay()`の自己デッドロックを解消し、UIコマンドキューの音量／パン／Mute／Solo消費、追加Trackのprepare、Solo抑制をC++オーディオ経路へ追加。

## 2026-08-10 第2回サブエージェント監査

3方向の読み取り専用調査を再実施した。以下の追加項目はコード編集を行わず、現行経路との接続状態を確認した結果である。

## 最優先（プレビュー版を名乗る前に修正）

### 1. UIモデルとエンジン状態の不一致（解決済み）

- 起動時の固定データはエンジン状態同期で置き換え、空状態では空モデルにするよう修正済み。[`slint_ui.rs`](../aura-ui/src/slint_ui.rs)

### 2. 波形の初回表示とキャッシュ後処理

- 波形キャッシュ生成開始後、初回取得が空配列を返す。[`aura_unified_engine.cpp:826-868`](../src/core/aura_unified_engine.cpp:826)
- UI側は周期ポーリングで生成完了後に再取得するため、初回空配列は次回ポーリングで更新される。
- `tid`を無視してリージョンIDだけで検索している。[`aura_unified_engine.cpp:826`](../src/core/aura_unified_engine.cpp:826)
- リージョン削除後も `peaks_cache_*.bin` のディスクファイルは残る。[`aura_unified_engine.cpp:913-939`](../src/core/aura_unified_engine.cpp:913)

### 3. 保存したサンプルレートのOpen復元（解決済み）

`ProjectSerializer`から読み込んだ`state.sampleRate`を`prepareToPlay`へ渡して復元するよう修正済み。[`aura_unified_engine.cpp`](../src/core/aura_unified_engine.cpp)

Track/Regionのcontrol/audio thread競合は、`Track`がcontrol threadでimmutable snapshotを構築し、audio threadがatomic公開された世代を読む方式へ変更済み。旧世代の回収は停止時のみcontrol threadで行う。[`track.hpp`](../src/core/engine/track.hpp)

### 4. 起動失敗の安全な表示（解決済み）

`AuraCore::new()`の失敗をUIへ表示してイベントループを維持するよう修正済み。[`slint_ui.rs`](../aura-ui/src/slint_ui.rs)

## 高優先（実用DAWとして未完成）

### オーディオ／レンダリング

- `sampler.rs`のClassic／Granular／Additive／Spectralは実サンプル入力ベースの最小処理を実装済み。ピッチ、ゾーン選択、サンプルレート変換を含む実オーディオグラフ統合は未完了。[`sampler.rs`](../aura-core-bridge/src/sampler.rs)
- `unified_engine.rs::render_block`は入力検証とRT安全な統括メタデータ記録を実装済み。Rust側からC++オーディオグラフを呼び出す完全統合は未完了。[`unified_engine.rs`](../aura-core-bridge/src/unified_engine.rs)
- Rust側のbounce／offline renderはキュー登録だけで、実グラフのレンダー接続が未完了。`export.rs`はPCM16 WAVの実ファイル書き出し基盤まで実装済み。[`bounce_core.rs`](../aura-core-bridge/src/bounce_core.rs)、[`export.rs`](../aura-core-bridge/src/export.rs)、[`offline.rs`](../aura-core-bridge/src/offline.rs)
- 既存の非同期選択範囲APIは、実グラフ未接続のため`RendererNotConnected`を明示する。一方、`process_region_to_wav`で入力済みPCMの範囲書き出しは実装済み。実オーディオグラフのレンダー結果接続は未完了。[`selection_based_processor.rs`](../aura-core-bridge/src/selection_based_processor.rs)
- `unified_engine.rs::render_block`は出力バッファを持たないRust側統括境界で、ブロック検証と進行メタデータを記録する。音声出力は現行C++レンダーが所有しており、Rustからのグラフ統合は未完了。[`unified_engine.rs`](../aura-core-bridge/src/unified_engine.rs)
- `sampler.rs`は4モードの最小処理を実装済み。実際のノートイベントからゾーンを選び、再生レートを制御する統合は未完了。[`sampler.rs`](../aura-core-bridge/src/sampler.rs)
- `vocal.rs`と`holographic.rs`は最小DSPを実装済み。ただし高度な音素解析・HRTF畳み込み・実オーディオグラフ統合は未完了。[`vocal.rs`](../aura-core-bridge/src/vocal.rs)、[`holographic.rs`](../aura-core-bridge/src/holographic.rs)
- `hardware.rs`は登録済みプロトコルのイベントを正規化して返す最小経路を実装済み。実MIDI／OSC受信、コントロールIDのプロトコル別マッピング、UI／オーディオグラフ接続は未完了。[`hardware.rs`](../aura-core-bridge/src/hardware.rs)
- `parameter_smoother.rs::process`は実装済み。係数計算とNaN入力拒否の回帰テストを追加済み。[`parameter_smoother.rs`](../aura-core-bridge/src/parameter_smoother.rs)
- `aura-audio-engine/`と`aura-core-bridge/src/modules/`には未接続の別実装があり、音声コールバックで使うとMutexとアロケーションが発生する。[`modules/audio_engine.rs:306-350`](../aura-core-bridge/src/modules/audio_engine.rs:306)

### UI callback

以下はSlintから呼ばれるがRust側の接続がない、または実質UI状態だけを変更する候補。

- `toggle_phase`, `toggle_dim`
- `plugin_param_changed`, `set_filter`, `set_snap`, `set_route`, `select_track`
- `reset_peaks`, `genesis_reset`, `apply_harmony`
- `area_select`, `automation_point_moved`, `macro_changed`, `marker_moved`, `scrub_to_marker`

宣言・呼び出し元は[`aura_studio.slint:66-137`](../aura-ui/ui/aura_studio.slint:66)、接続実装は[`slint_ui.rs:534-1158`](../aura-ui/src/slint_ui.rs:534)で照合する。

録音・アーム・FXはcallbackを接続済みだが、現行C++エンジンの録音入力／FXプロセッサへは未接続で、UIのアクション履歴に明示する。ルーティング・プラグインパラメータ・オートメーションは未接続が残る。実API未整備の項目は、UIのみの状態変更に留めず未対応表示へ分離する。

### 数値安全性・グラフ整合性

- `pitch.rs`の周波数0／非有限値伝播は修正済み。ターゲット音域、比率、ブロック定義を検証する回帰テストを追加。[`pitch.rs`](../aura-core-bridge/src/pitch.rs)
- `graph_solver.rs`の循環依存・未知依存・キャッシュ重み順は修正済み。残る課題は、現行C++オーディオグラフがこのRustソルバーを実際に呼ぶ統合である。[`graph_solver.rs`](../aura-core-bridge/src/graph_solver.rs)

### ダミー表示

- サンプルブラウザは起動時空カタログで、実際にプレビューした音源をファイル情報付きで追加する。[`slint_ui.rs`](../aura-ui/src/slint_ui.rs)
- レイテンシはエンジンのブロックサイズ／サンプルレートから計算し、バッファサイズも実値を表示する。[`slint_ui.rs`](../aura-ui/src/slint_ui.rs)、[`aura_studio.slint`](../aura-ui/ui/aura_studio.slint)
- プラグインUIが `Mocked for Industrial Aesthetics`。[`aura_studio.slint:2917-2924`](../aura-ui/ui/aura_studio.slint:2917)
- JS-REPL、Bridge状態、プラグイン数、メモリ表示が固定値。[`aura_studio.slint:4152-4165`](../aura-ui/ui/aura_studio.slint:4152)

## プレビュー後でよいが未実装

### プラグイン

- 外部VST3/AU/CLAPの実インスタンス生成なし。ただしトラックへのプラグインスロット登録は実装済み。[`vst3_host_processor.hpp:39-56`](../src/core/plugins/vst3_host_processor.hpp:39)、[`track.hpp`](../src/core/engine/track.hpp)
- `process`はバイパス。[`vst3_host_processor.hpp:31-35`](../src/core/plugins/vst3_host_processor.hpp:31)
- サンドボックスのプロセス起動・監視なし。[`plugin_host.hpp:108-117`](../src/core/plugin_host.hpp:108)
- プラグインスキャンは固定の `Aura Pro EQ`。[`plugin_host.hpp:112-117`](../src/core/plugins/plugin_host.hpp:112)

### GPU

- Vulkanは初期化止まりでswapchain、command buffer、submit/presentなし。[`vulkan_kernel.cpp:71-157`](../src/graphics/platform/vulkan_kernel.cpp:71)
- Metalも多くのprimitiveが空実装。[`metal_kernel.mm:133-156`](../src/graphics/platform/metal_kernel.mm:133)
- これらの描画backendはSlintの起動経路へ接続されていない。[`main.rs:10`](../aura-ui/src/main.rs:10)

### その他DSP／機器

- `vocal.rs`と`holographic.rs`の最小DSPは実装済み。高度な音素解析・HRTF畳み込み・実オーディオグラフ統合は未完了。[`vocal.rs`](../aura-core-bridge/src/vocal.rs)、[`holographic.rs`](../aura-core-bridge/src/holographic.rs)
- WAVデコーダはPCM16／24／32とFloat32を検証付きで読み込み、短い入力や不正チャンクを`Result`で拒否する。[`preview_audio_runtime.rs`](../aura-core-bridge/src/preview_audio_runtime.rs)
- モジュレーターのRandomとEnvelopeは実装済み。残る課題は実際のオーディオグラフからこの行列を呼び出す統合である。[`modulator_system.rs`](../aura-core-bridge/src/modulator_system.rs)
- MCU／HUI／EuCon／OSCの共通イベント正規化は実装済み。実デバイス受信とプロトコル固有のマッピングは未完了。[`hardware.rs`](../aura-core-bridge/src/hardware.rs)
- 複数のaudit APIが常にtrueを返し、未接続処理を成功扱いする。[`sampler_engine.rs:241`](../aura-core-bridge/src/sampler_engine.rs:241)

## 実装順の提案

1. 波形キャッシュのディスクファイル削除と完了通知を整理
2. 未接続callbackを「実装」または「未対応として無効化」に分類（録音／Arm／FXは明示表示まで実施）
3. Rust側のオフライン／選択範囲処理をC++実レンダーへ集約
4. Rustモジュレーション行列と現行オーディオグラフを統合
5. 外部プラグイン、Vulkan、別OSデバイスはプレビュー後に独立フェーズで実装

## 判定

現状は「ビルド可能なUI＋トラック／WAV／保存／基本バウンス経路」であり、完全なDAWではない。固定UIモデル、Save/Openのサンプルレート復元、波形の周期再取得、C++所有の非同期バウンス、基本Trackコマンド消費、CoreAudio実デバイス設定、アセット統合、Track/Region snapshotは対応済み。残る受け入れ阻害要因は、Render開始callback、保存の完全対称化、Rust別経路、外部プラグインである。
