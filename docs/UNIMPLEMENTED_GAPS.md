# Aura DAW 未実装・未接続一覧

調査日: 2026-08-11  
調査方法: Rust／C++／FFI／Slintを3方向のサブエージェントで読み取り専用監査し、現行ソースと突合。

> **履歴資料:** このファイルは2026-08-11時点の監査スナップショットです。
> 現在の対応状況を表す正本ではありません。現行の公開範囲・検証条件・
> capabilityの判定は [`RELEASE_READINESS.md`](RELEASE_READINESS.md)、
> [`ARCHITECTURE_STATUS.md`](ARCHITECTURE_STATUS.md)、および
> `scripts/verify_release_bundle.sh` を参照してください。以下の項目には、
> その後に実装済みになったものも含まれます。

## 今回解消した重大項目

## 2026-08-11 並列監査の統合結果

Rust/C++の未実装、RT安全性、UI/FFI/デバイス導線を3系統で読み取り専用監査した。今回の監査ではファイル変更を行わず、既存のdirty worktreeも保持した。

### 最優先（P0）

- `src/io/audio_drivers.hpp:44-52`: 旧`DriverFactory::createDefault()`がmacOS／Windows／Linuxの全環境で`DummyAudioDriver`を返す。`initialize()`／`start()`も無条件成功のため、旧抽象化経路を使うと必ず無音になる。Silent fallbackは明示指定時だけ許可し、実バックエンド失敗はエラーとして返す。
- `src/core/driver/mac_audio_driver.hpp:139-163`: `stop()`／device listenerがcallback完了を待たずにAudioUnitを破棄し得た。in-flight counterと停止→quiescence→破棄の順序を実装済み。デバイス消失もatomic状態で通知する。
- `src/io/audio_driver_mac.hpp:79-100`: 固定入力バッファと1本のAudioBufferListで範囲外書き込み・ステレオレイアウト誤認の危険があった。2ch非インターリーブ入力、最大フレーム検証、Render失敗時無音化、初期化失敗時cleanupを実装済み。
- `src/core/engine/effect_chain.hpp:20-41`: `EffectChain`の`process`／`addProcessor`／`clear`がno-op。実装済みの`src/core/effect_chain.hpp`へ統合するか、未接続型を登録経路から排除する。

### 高優先（P1）

- `src/core/engine/effect_chain.hpp:43`／`src/core/engine/track.hpp:172-181`: RT callback内のmutex、vectorコピー、shared_ptr参照カウント操作。control threadでimmutable snapshotを構築し、callbackはatomic世代参照だけにする。
- `src/core/aura_unified_engine.cpp:314`／`src/core/log_buffer.hpp:38`: callback内で文字列コピー・timestamp・ログ書き込みを行う。callbackはatomic flag/counterだけ更新し、詳細ログはcontrol threadで回収する。
- `aura-core-bridge/src/modules/audio_engine.rs:46,306`: 将来RT経路へ接続するとVec確保、Mutex、ファイル処理、NaN伝播が発生する。Rust RT APIを事前確保済みsliceとlock-freeイベントに限定し、DSP境界でfinite検証する。
- `src/dsp/synthesis/sampler_mapping_engine.hpp:34-47`: `resolveSample()`が常に空、`addZone()`がno-op。MIDIノートからサンプルを解決できないため、zone registryとnote/velocity範囲検証を実装する。
- `aura-core-bridge/src/library.rs:23-35`: 実ファイルを走査せず固定アセット1件を登録。`read_dir`、メタデータ、拡張子、存在確認による実走査へ置換する。
- `aura-core-bridge/src/resources.rs:30-52`: scan/consolidateが空、auditが無条件true。既存resource managerへ統合し、欠損・サイズ・重複を検査する。
- `aura-core-bridge/src/session_launcher.rs:31-51`: BPM／sample rate未検証でゼロ除算・`% 0` panicの可能性。有限値・正値検証とResult化が必要。
- `src/scae/vocal_restoration.hpp:114-124`: Vocals以外を全ゼロにする固定ダミー。未接続なら成功扱いせず、明示的な未対応エラーを返す。
- `src/core/aura_unified_engine.cpp:1059-1061`: `browser_preview()`が引数を無視して何もしない。実プレビュー登録・デコード・再生キュー接続が必要。
- `src/synthesis/aura_sampler_pro.hpp:247-248`: `updateVoiceCC()`が空。CC/MPEをvoice単位へ伝播する必要がある。
- `aura-ui/ui/aura_studio.slint:66-137`／`aura-ui/src/slint_ui.rs:608-1665`: `automation_point_moved`、`area_select`、`plugin_param_changed`、`set_route`等のcallbackがUIに存在するがRust接続なし。編集操作が見た目だけで終わる。
- `aura-ui/ui/aura_studio.slint:3128,3541-3546`: 空プロジェクト・削除直後に`tracks[sel_idx]`を直接参照する。空モデル用の安全な選択状態が必要。
- `aura-ui/src/slint_ui.rs:322-336`／`src/core/aura_unified_engine.cpp:942-968`: 波形初回取得で同期デコードしUIをブロックする。Open後はplaceholderを表示し、デコードとピーク生成を完全にworkerへ移す。

### 中優先（P2）

- `src/dsp/iprocessor.hpp:60-70`: state/parameter APIが空、取得値0、automation常時true。未対応を明示する`supports*`契約へ変更する。
- `src/core/gpu_audio_kernel.hpp:29-31`: float pointer版`processFXChain`が空でAudioBuffer版と二重化。APIを統合する。
- `src/core/dsp/effects/procedural_foley_kernel.hpp:15-23`: trigger/processが空。未対応ならUIから隠し、実装時はイベントキューと出力を接続する。
- `src/graphics/ui_components/aura_pro_ui.hpp:103-105`: `DummyInspector::render()`が空。実状態を描画するか未対応表示にする。
- `aura-ui/ui/aura_studio.slint:2916-2924,3040-3044,4183,4310-4318`: plugin状態、sample rate、latency、memory、LUFSが固定表示。実エンジン値へ接続し、未取得時は`N/A`表示にする。
- `src/core/aura_unified_engine.cpp:1024-1034`: 波形cacheの一時ファイルがカレントディレクトリに残る。cache root、世代、削除／GCを定義する。

### 導線の現状判定

- `scripts/run_preview.sh`を追加。releaseバイナリの存在を検証し、未指定時はSoftware描画でプレビューを安定起動する。`SLINT_BACKEND`指定時は明示された描画backendを使用する。

- Open、Save、再生/停止、基本波形取得、playheadは接続済み。
- 外部VST3/AU/CLAPの実instantiate/processは未接続。
- デバイス切断後の停止・再接続は、CoreAudio通知→再接続要求→明示的`try_reconnect_audio_device()`で再構築する経路を実装済み。UIからの定期ポーリングとPause表示は残課題。
- ステム分離、Sampler mapping、旧DriverFactory、複数UI編集callbackは未完了。

- `src/dsp/analysis/drum_replacer.hpp`: 入力バッファのトランジェント検出からMIDIノートを生成し、サンプル位置付きの決定論的イベントストリームを書き出す実装へ置換。

- `prepareToPlay()`の`m_tracksMutex`自己デッドロックを解消。
- UIから投入された音量・パン・ミュート・ソロのコマンドを、オーディオブロック冒頭で消費するよう修正。
- 追加トラックを現在のサンプルレート／ブロックサイズでprepareしてから公開。
- ソロ状態をTrackへ保持し、ソロトラック以外を安全に抑制。
- WAVデコード、サンプル処理、選択範囲WAV出力、ハードウェアイベント正規化を実装。
- 固定サンプルブラウザ、固定レイテンシー、固定バッファ表示を実データベースへ移行。
- CoreAudioから実デバイスのサンプルレート／バッファフレーム数を取得し、エンジンのprepare/startへ反映。
- `AssetOrchestrator::consolidate_assets`を実装し、参照アセットのコピー、パス・サイズ更新、失敗理由、重複・衝突を処理。
- サンプルカタログが実ファイルパスを保持して再プレビューできるよう修正し、空トラックへのImportを拒否。
- バウンス形式・出力パスの検証と、プロジェクト保存のflush／書き込み結果検証を追加。
- `Track::process()`をcontrol threadの`m_regions`直接参照からimmutable Region snapshot参照へ変更。世代はatomic公開し、旧世代はRTスレッドで解放せず停止時に回収。
- Regionのgain／fade編集をtyped setter経由にし、編集後のsnapshot公開漏れを防止。
- Rust側のProjectDocumentを追加し、メタデータ、サンプルレート、トラック、リージョン、ゲイン、フェード、Mute/Soloを同一JSONで原子保存・検証付き復元。
- Render UIをキュー投入済み／処理中／完了／失敗／キャンセル未接続に分け、進捗未接続時に偽のパーセント表示をしないよう修正。
- C++バウンスにatomicな状態、進捗、キャンセル要求APIを追加。RTコールバックをブロックせず、Idle／Queued／Rendering／Completed／Failed／Cancelledを区別。
- C++バウンス状態をAudioEngine wrapper、CXX bridge、AuraCore、Slint Timerへ接続。UIが実進捗・完了・失敗・キャンセルを表示し、キャンセル要求を送信可能。
- Sidechain Followerが入力バッファの有限値RMSを使うようになり、空入力・NaN/Inf・異常depthを安全に処理。
- AU/VST3/CLAPのシステムスキャンを実装し、ユーザーパス展開、権限エラー、重複、拡張子、Info.plistメタデータを安全に処理。
- Plugin registryの登録・一覧・存在確認・未実装instantiateの失敗理由を実装し、ロード成功と誤認しないよう明示。
- PluginSandboxHostの基底DSP APIとの`noexcept`契約不一致を修正。
- 内蔵Aura/LimiterをTrackのEffectChainへ接続し、prepare・ブロック境界同期・音声処理を実装。外部形式は追加失敗として扱う。
- `ParameterAutomatorPro`がplugin/parameterごとの値を保持し、有限値検証・0..1クランプ・安全な取得・クリアを提供。
- `PluginScanner::scanInSandbox`を入力検証と状態管理へ変更し、空path・不存在・directory・非通常ファイルを明示的に拒否。実プロセス起動は未実装として偽装しない。
- UIのSCAN操作を実ファイルシステム走査へ接続し、発見件数・拒否件数・未ロード状態をLast Actionへ表示。
- AudioEngine／AuraCoreの各ハンドル破棄で共有singletonをshutdownしないよう修正し、プロセス終了用の明示的な`shutdown_process()`を追加。
- Preview sampleを制御側のappend-only世代ストレージとatomic raw pointerへ変更し、audio callback内のshared_ptr refcount／解放を排除。
- SubBassGeneratorのno-op processを実入力処理へ置換し、LPF/DCブロック、エンベロープ、ゼロクロス基音推定、1オクターブ下のサイン生成、有限値保護を実装。
- DeEsserのlookahead placeholderを固定リングバッファへ置換し、先読み検出ゲインを遅延音へ適用。低SR、短いbuffer、NaN/Infを処理。
- ChannelStripのresetを実装し、ゲイン／パンの平滑化途中状態をターゲットへ同期。空のresetによる残留ランプを解消。
- 再生ヘッドのbeat変換にサンプルレート／BPMの有限値・ゼロ除算ガードを追加。
- 空間パンナーへ渡す座標を有限値検証し、±100の範囲へ制限。異常なUI/FFI入力のNaN伝播を防止。
- C++プロジェクト形式をv12へ更新し、トラック種別、Mute/Solo、3D座標をSave/Openで対称に保持。
- C++プロジェクト形式をv13へ更新し、Volume/Panオートメーションを有限値検証付きでSave/Open。
- C++プロジェクト形式をv14へ更新し、プロジェクトのrootNote／scaleTypeをSave/Openで保持。
- `StreamingSource`をraw float32ステレオの実データ読み出しへ変更。フレーム数、初回アクティブバッファ、短い読み込み、EOF、境界時の無音化を処理。
- `Track::addRegion()`と`Track::process()`でRegionの長さを音源バッファへ制限し、空音源・不正ゲイン・過大フェード・64-bit位置計算のオーバーフロー・範囲外読み出しを拒否／無音化。
- オーディオコールバックのin-flight数を追跡し、shutdown／停止時にquiescenceを待ってから回収するよう変更。削除・再prepare時のTrack遅延解放はshutdownまで保持し、`m_isPlaying == false`だけでUAFを判断しない。
- TrackのEQ段で入力・ゲイン・内部状態・出力を有限値検証し、NaN/Inf発生時に状態をリセット。異常なdB入力も範囲制限して、次ブロックへ破綻を持ち越さない。
- `RecordingEngine`の録音キューをfloat単位からステレオフレーム単位へ変更。片チャンネルだけpushされるオーバーフローでL/Rのインターリーブが崩れる問題を防ぎ、WAVデータサイズをフレーム数と一致させた。
- `RecordingEngine`がディスク書き込み失敗をatomic状態として保持し、writerスレッド停止後も`stop()`が必ずjoin・ヘッダー更新・ファイルクローズを行うよう修正。録音失敗後のスレッド残留と成功誤認を防止。
- `src/dsp/synthesis/wavetable_oscillator.hpp`の常時0.0スタブを実装。Sine/Sawテーブル、周波数Mip選択、Catmull-Rom補間、モーフ、位相更新、有限値保護をRTアロケーションなしで処理。
- `src/dsp/effects/console_dsp.hpp`の空processを実装。入力／サイドチェイン検出、VCA型attack/release、threshold/ratio、drive付きtanh飽和、有限値保護を追加。
- `src/dsp/effects/pitch_corrector.hpp`の空processを実装。ブロック内ゼロクロス推定、最近傍半音スナップ、補正量平滑化、既存PitchShifter接続、未検出時の安全なパススルーを追加。
- `src/core/engine/scale_system.hpp`のscale／chordスタブを実装。Major／Minor等の正しい12音マスク、rootを考慮した最近傍量子化、入力検証付きコード履歴を追加。
- `aura-core-bridge/src/group_manager.rs`と`vca_manager.rs`の空管理処理を実装。グループ双方向所属、属性伝播、整合性監査、VCAゲインの有限値検証、トラック割当、階層ゲイン解決を追加。
- C++の`GroupManager`／`VCAManager`／`VCAControlSystem`にも所属管理、属性同期、VCAゲイン解決を実装し、Rust側だけに処理が閉じる状態を解消。
- Trackのブロック処理からC++ `VCAManager`の解決済みゲインをO(1) atomic lookupで参照し、エフェクトチェーン後のTrack信号へ適用。

検証済み: `cargo build --release -p aura-ui` 成功、Rustワークスペーステスト61件成功、`track.hpp` Clang構文検証成功。

## 追加監査で判明した未実装・未接続項目

サブエージェント3系統による読み取り専用監査（UI/FFI、RT安全性、DSP/IO）を実施。以下は既存項目と重複しない、または優先度を引き上げるべき項目。

### P0: 実音声が無音になる経路

- `StreamingSource`の常時無音と初回非アクティブバッファ問題は解消済み。残る課題は複数チャンネル／WAVヘッダ付きストリーミングへの対応。
- `src/platform/audio_device.hpp`／`src/platform/macos/coreaudio_device.mm`: macOSの共通`createAudioDevice()`を実CoreAudioデバイスへ接続。非Appleでは未接続のネイティブバックエンドをSilentへ明示的にフォールバックし、初期化前のstartを拒否する。残課題はWindows WASAPI／Linux PipeWire実装。

### P0: オーディオコールバック寿命（主要経路は解消済み）

- `src/core/aura_unified_engine.cpp`でcallbackのin-flight数とquiescence待機を導入し、削除・再prepare時のTrack解放をshutdownへ延期済み。残課題はCoreAudioのstop完了通知とエンジンquiescence待機を同一状態機械へ統合すること。
- `src/core/engine/track.hpp:143-150` はRT上でatomic shared_ptrロードを行い、参照カウント操作／解放がcallbackへ現れる可能性がある。
- `src/core/effect_chain.hpp:43-59` はRT上で`try_lock()`を使用。待機しないが、標準mutex依存はRT保証として不十分。

### P1: 完全な空実装・偽成功

- `src/rendering/metal/waveform_compute.hpp`: Metal pipeline、buffer、dispatch、ピーク取得APIを実装。GPU初期化・コマンド実行に失敗した場合は有限値を保つCPUフォールバックへ切り替える。残課題は非同期dispatchと長時間波形のLODキャッシュ統合。
- `src/dsp/effects/console_dsp.hpp`: 基本DSPを実装済み。残課題は実機測定に基づくコンソール特性の校正と、専用IProcessor／EffectChainへの統合。
- `src/dsp/effects/pitch_corrector.hpp`: 基本ピッチ補正を実装済み。残課題はYIN等の高精度検出、スケール指定、フォルマント保持。
- `src/dsp/synthesis/wavetable_oscillator.hpp`: 実装済み。残課題は実測帯域制限用のより厳密なMip生成と、ユーザー波形のロードAPI。
- `src/dsp/analysis/drum_replacer.hpp`: Audio-to-MIDIは実装済み。現時点では汎用トランジェント検出を使うため、キック/スネア等の音色分類やユーザー調整可能な検出プロファイルは未対応。
- `src/dsp/analysis/dynamics_ai.hpp`: 入力の有限peak/RMS/crest factorを解析し、決定論的なthreshold／ratio／attack／release提案を返す実装へ置換。残課題はUI表示と実コンプレッサーへの適用接続。
- C++／Rust双方のGroup/VCA基本管理・伝播・階層解決を実装済み。残課題はGroup属性によるMute／Solo／Pan等のTrack操作伝播と、プロジェクト保存への所属情報追加。
- VCAゲインのTrack音声適用まで実装済み。残課題はGroup属性によるMute／Solo／Pan等のTrack操作伝播と、プロジェクト保存への所属情報追加。
- `src/core/engine/scale_system.hpp`: スケール量子化とコード履歴を実装済み。残課題はUI／MIDIイベント経路への接続とコード履歴の公開API。
- `aura-core-bridge/src/parallel_bounce_orchestrator.rs`: 重複排除付きの決定論的タスク計画、タスク取得、完了通知、監査を実装。残課題はこの計画を実際のオーディオレンダラー／WAVエンコーダへ接続すること。未接続時にレンダリング成功とは報告しない。
- `src/io/mmap_audio_file.hpp`: 空ファイル、mmap失敗、RIFFチャンク破損、未整列データ、範囲外サンプル、NaN/Infを検証。16/24/32-bit PCMと32-bit floatを安全に読み出し、例外時もマッピングとFDを解放。
- `aura-core-bridge/src/session_launcher.rs`: BPM/sample rateの有限値・正値検証、quantization幅のゼロ回避、重複clip監査、境界起動テストを実装。
- `src/io/audio_drivers.hpp`: 旧Dummyドライバを明示的なSilent fallbackとして状態管理。初期化前start、不正sample rate、ゼロbufferを失敗扱いにし、状態・エラー取得APIを追加。
- `src/core/engine/effect_chain.hpp`: no-opの二重実装を廃止し、動作する`src/core/effect_chain.hpp`へalias統合。プロセッサ追加・clear・block処理が実装経路へ到達する。
- `src/core/driver/mac_audio_driver.hpp`: callbackのin-flight数を追跡し、`AudioOutputUnitStop`後にquiescenceを待ってからAudioUnitを破棄する停止順序へ変更。
- `src/platform/macos/coreaudio_device.mm`: 共通AudioDevice経路にもcallbackのin-flight待機、停止中の無音化、Renderエラー検出、NULL/不正バッファ保護、開始失敗時cleanupを追加。
- `aura-ui/src/slint_ui.rs`: transportの再生状態をCoreを正とする同期へ変更し、再生ヘッドの有限値検証、automation表示切替、automation pointの範囲・値検証付きUI反映を追加。
- `src/core/effect_chain.hpp`／`track.hpp`／`aura_unified_engine.*`／`audio_engine.hpp`／`aura-ui/src/slint_ui.rs`: 内蔵Auraプラグインのparameter変更をUIからTrackのEffectChainへ接続。外部形式・存在しないFXは失敗表示し、成功と誤認しない。
- `src/core/engine/macro_control_manager.hpp`: no-opだったmacro mapping登録・反転・範囲変換・重複更新を実装し、非有限値と範囲外入力を拒否。
- `src/core/aura_unified_engine.*`／`audio_engine.hpp`／`aura-core-bridge/src/lib.rs`／`aura-ui/src/slint_ui.rs`: Macro変更をUIからCoreのMacroControlManagerへ接続し、UI表示モデルも正規化値へ更新。
- `src/core/engine/routing_engine.hpp`／`aura_unified_engine.*`／`audio_engine.hpp`／`aura-core-bridge/src/lib.rs`／`aura-ui/src/slint_ui.rs`: no-opだったroute登録をCoreへ接続。重複更新、解除、自己接続拒否、UI結果表示を実装。
- Track処理へ多段の実音声ルーティングを接続。固定長・atomic公開のトポロジー順を制御側で構築し、オーディオスレッドでは入力加算→Track処理→次段送信を行う。登録時のCycle検出、未コンパイル時の安全なフォールバック、BusTrack生成・BusSystem登録、Bus pre-FX集約→処理→post-FX公開、Bus専用EffectChain、SidechainCompressorの実処理、plugin index単位のsidechain注入、UI/FFIからのlink設定、Track削除時の解除、再prepare後のsource buffer自動再公開まで対応済み。feedback routeも通常Cycleとは分離した固定1ブロック遅延経路として実装済み。任意サンプル遅延、feedback量の自動安定化は未完了。
- `src/dsp/synthesis/sampler_mapping_engine.hpp`: note/velocity範囲を検証するzone registryとサンプル解決を実装。
- `aura-core-bridge/src/library.rs`: 固定ダミーアセットを廃止し、音声拡張子・存在・サイズ・重複を検証する再帰ファイル走査へ移行。
- `aura-ui/ui/aura_studio.slint`／`aura-ui/src/slint_ui.rs`: 空トラックモデル時のInspector／Plugin表示と選択行を安全化。`tracks[sel_idx]`の範囲外評価を抑止。

### P1: デバイス・入力整合性

- `src/core/driver/mac_audio_driver_host.mm:112-180` と `mac_audio_driver.hpp:139-190`: デバイス消失／default output変更を通知し、既存AudioUnitを安全に停止・破棄後、最新のsample rate／buffer sizeを再取得して再prepareする再接続経路を実装済み。残課題はUIのPause表示と定期ポーリング接続。
- `src/core/engine/track.hpp`: Regionの長さ、フェード、ゲイン、AudioBuffer境界、64-bit位置計算を入口と再生処理で検証済み。残課題は、シリアライザ以外の全Region生成経路にも同じ入力契約を適用すること。
- `src/core/recording_engine.hpp`: ステレオフレーム単位のSPSCキューへ移行済み。残課題はデバイス切断時の再接続・ユーザー通知の統合。
- `src/core/recording_engine.hpp`: ステレオフレーム単位のSPSCキューへ変更済み。残課題は録音開始／停止のUI・Trackへの実接続と、ディスク書き込み失敗のUI通知。
- TrackのEQ内部状態（`m_lowStateL/R`）と入出力の有限値検証・リセットを実装済み。残課題は各外部DSPプラグインにも同じ状態回復契約を適用すること。

### P1: UI callback未接続

`aura-ui/ui/aura_studio.slint:66-117`の宣言に対し、Rust側登録がない主要callback:

- `plugin_param_changed`, `set_filter`, `toggle_phase`, `set_route`, `macro_changed`
- `loop_changed`, `set_tool`, `set_snap`, `set_track_delay`, `set_time_sig`
- `piano_roll_note_click`, `automation_point_moved`, `marker_moved`, `move_selected`

Record/Arm/FXの一部はUI状態だけを変更し、エンジン未接続を表示している。プレビューでは未対応操作を無効化するか、明示的な状態表示を維持する必要がある。

### P2: 描画バックエンド

- `src/graphics/platform/vulkan_kernel.cpp:79-151` はデバイス初期化までで、Surface/Swapchain/CommandBuffer/Submit/Present/描画コマンドが未実装。Vulkan対応は現状「初期化のみ」。
- `src/graphics/platform/metal_kernel.mm:133-149`にもパス、スペクトログラム、シザー、ブラーの空処理が残る。
- `src/core/dsp/effects/master_limiter.hpp:26-43`はlookahead、sample rate、release係数の上限・有限値検証を追加すべき。

## P1: プレビュー版の前に判断・実装が必要

### 1. 実オーディオ経路の二重化

- `aura-core-bridge/src/modules/audio_engine.rs`のRustオーディオ処理は、実デバイスコールバックから呼ばれていない。
- 実デバイスはC++の`AuraUnifiedEngine::processBlockDirect()`へ接続されている。
- Rust側を正式な処理グラフへ統合するか、未使用の二重実装を整理して責務を一本化する必要がある。

### 2. RTスレッド上の`shared_ptr`解放

- Preview sampleのaudio callback内shared_ptrロード・参照カウント・解放を除去済み。
- 制御側が不変世代をengine lifetimeまで保持し、callbackはatomic raw pointerを読むだけにした。長時間のプレビュー世代増加に対する上限・回収は今後の最適化課題。

### 3. AudioEngine singletonのライフサイクル

- 各`AuraCore::Drop`とAudioEngineデストラクタは共有singletonをshutdownしないよう修正済み。
- 個別のdriver停止とプロセス終了用`shutdown_process()`を分離した。複数インスタンスでの実機ライフサイクル確認が残る。

### 4. Save/Openの完全対称化

- RustのProjectDocument経路はメタデータ、サンプルレート、トラック、リージョン、ゲイン、フェード、Mute/Soloを対称に保存・復元し、壊れたスキーマを拒否する。
- C++保存・復元はトラック種別、プラグイン状態、automation、空間状態を完全には保持しない。
- `TrackState`にtype、plugin、automation、spatial stateを明示し、Rust／C++の保存経路を最終的に一つへ集約する必要がある。

### 5. Export開始callbackと進捗

- C++側の進捗・状態・キャンセルAPIをAudioEngine wrapper、CXX bridge、AuraCore、Slint Timerへ接続済み。
- 実機での長時間バウンス中に進捗が単調増加し、完了・失敗・キャンセル表示が一致することの確認が残る。

## P2: プレビュー後または機能拡張フェーズ

### オーディオ／DSP

- Sidechain Followerは入力バッファの有限値RMSとdepthを使うよう実装済み。実デバイス経路での長時間動作確認は残る。
- Rustのbatch/offline renderはジョブ件数やメタデータ管理が中心で、C++実グラフへ未接続。
- Rustの`unified_engine`は統括メタデータのみで、音声出力はC++側が所有する。この責務をAPI上で明示する必要がある。
- 複数モジュールの`audit_*`が無条件に`true`を返すため、実際の接続状態・有限値・範囲を検査する必要がある。

### プラグイン

- `add_plugin()`は現在スロット番号をTrackに保持するだけで、DSPチェーンへ接続していない。
- 内蔵LimiterのpluginTypeはDSPチェーンへ接続済み。外部形式は未実装instantiateとして明示的に拒否する。
- システムスキャン、registry管理、未実装instantiateの明示エラー、実ファイル走査結果のUI表示まで対応済み。実機プラグインのinstantiateは未実装。
- VST3／CLAPホストはバイナリの解決までで、インスタンス生成・audio/MIDI process・state・latencyが未実装。
- AUも実ホスト処理、GUI埋め込み、クラッシュ分離が未完了。
- 外部プラグイン未接続の状態をUI/APIで明示し、ロード済みと誤認させない必要がある。

### UI操作

- `toggle_phase`、`set_filter`、`set_snap`、`toggle_dim`、`apply_harmony`、plugin parameter、macro、routeなどのcallbackが未接続またはUI専用。transport、automation表示、Volume/Pan automation pointのCore反映は対応済み。残りのplugin parameter・macro・routeと、それ以外のautomation laneは未接続。
- 録音・Arm・FXはUI callbackを接続済みだが、現在はエンジン未接続であることを表示するだけ。録音入力・processor bypassへの接続が必要。
- 空トラック状態でImport操作を押した場合のガードは実装済み。実機UI確認のみ残る。
- サンプルカタログは実パスを保持して再Previewできるよう修正済み。実機UI確認のみ残る。
- Marker、VCA、FXモデルの固定値をProject/Core状態へ同期する必要がある。

### GPU／可視化

- `aura-ui`の通常描画はSlintのRectangle/Path中心で、Vulkan/Metal独自経路は起動・描画へ接続されていない。
- 一部Visualizerと波形はsin波・固定値・空Pathを表示する。
- 実波形、FFT、transientデータを描画モデルへ渡し、データなし時だけEmpty Stateを表示する必要がある。

### アセット統合

- `asset.rs`のconsolidate処理は参照抽出、プロジェクト配下へのコピー、パス・サイズ更新、失敗理由、重複・衝突処理まで実装済み。実プロジェクトでの確認のみ残る。

## プレビュー判定への影響

現時点でビルドと単体テストは通るが、次の実機確認が未完了なら一般配布は不可。

1. CoreAudioの実サンプルレート／バッファサイズで再生する。
2. Save/Open後にトラックと音声が維持される。
3. 追加トラック、音量、パン、Mute/Soloが音声へ反映される。
4. Render開始が実際のC++バウンスへ到達する。
5. 30分以上の再生で音切れ、UAF、メモリ増加がない。

## UI/UXの実用DAW化（2026-08-11）

今回、機能名と主要画面の表示を標準的なDAW用語へ整理した。`AURA SOVEREIGN`、`QUANTUM LOCK`、`DIVINE`、`VOID` などの演出優先のラベルを、`AURA DAW`、`LOCK TRACK`、`MASTER CONSOLE`、`FOCUS MODE` など、操作内容が読んだ瞬間に分かる表記へ変更した。

- トップメニューを`FILE / EDIT / TRACK / MIX / PLUGIN / WINDOW / HELP`へ統一。
- 新規プロジェクト画面、ルーティング、バージョン履歴、プロジェクト状態、空間音響の見出しを実用語へ変更。
- レンダリング中の意味のないサイン波アニメーションを固定表示へ変更。再生位置・メーターなど実データに由来する表示とは区別する。
- アシスタント機能は削除せず、`ASSISTANT`／`ASSIST`として補助機能の位置付けを明示。プロダクション画面の主役にしない。

残課題は、Slint単一ファイルの分割、実波形／FFTデータの描画接続、非本番オーバーレイの完全な隔離、UI実機操作確認である。

### UIテーマ基準

- 共通背景は`#202020`、パネルは`#2D2D2D`、境界は`#181818`〜`#3A3A3A`を基準にする。
- 選択はApple Blue系、オーディオ状態はミューテッドグリーン、Soloはオレンジ、録音・警告は赤に限定する。
- 本文は`SF Pro Text`、時間・BPM・レベル値は`SF Mono`を優先する。
- 共通ボタンは22px高、3px角丸、薄い縦グラデーションと1px境界線を使う。発光、過度な丸角、理由のないアニメーションは追加しない。
- `aura_studio.slint` の`side_tab == 4`重複表示を解消し、タブ名から絵文字を外して役割を明確化。
- `industrial_styles.slint` の旧ネオン配色も共通のチャコール／機能色へ同期。現時点でIndustrialStyle系ファイルはメイン画面から未接続であり、循環importは確認されていない。
- 本番サイドバーを`Track / Browse / Status / Settings`の4タブに限定し、旧Plugin Rackの重複表示を削除。下部ビューは`Mixer / Editor`を主導線とし、Piano Roll等は既存コードをADV側へ残す。
- 最新UI整理では本番サイドバーを`Inspector / Library`の2タブへさらに縮小し、下部を`Mixer / Audio Editor / Piano Roll / Score`の4ビューへ整理。旧モード用の状態・実装は互換性のためADV側に保持する。
- 共通ボタンを最低60x24px、ノブ表示を48px、コンパクトフェーダーを24x160pxへ拡大。小画面での実機確認は未完了。
- UI共通層を`ui/style.slint`（唯一のStyle）と`ui/components.slint`（共通ノブ／ボタン）へ切り出し、`aura_studio.slint`から重複定義を除去。旧`industrial_styles.slint`は削除し、独立部品はcanonical Styleをimportする構成へ変更。
- `Z_Miracle`を削除し、実用名の`Z_AudioEngineStatus`として`ui/panels.slint`へ移動。現在の分割単位は`style / components / panels / main`。
- MixerStripの`ai_status`／`ai_detail`とTrackモデルの同名フィールドを削除。上部の情報表示はネットワークフィードではなくCPU・遅延・Diskの1行ステータスへ統合した。
- ショートカットは再生、ループ、Undo、Redo、Open、Save、Command Palette、Inspectorの8系統へ整理。フェーダーとノブはホバー時にアクセント表示し、操作対象を明示する。
- `Z_SoulCore`、`Z_SoulSignature`、`Z_SentimentMeter`を削除。感情値・魂色・演出用パルスを本番UIの状態モデルから排除し、旧アドバンスト画面の表示は`Z_AudioEngineStatus`または通常のテキストへ置換した。
- 今後のUI状態は、再生、録音、エンジン状態、CPU、遅延、選択トラックなどユーザー操作に必要な値だけを保持する。意味のないタイマー演出や詩的な状態名を新規追加しない。
- `ui/style.slint`へLogic系の中間色、ベゼル、LCD用カラー、機能別アクセントを追加。背景はチャコール、状態色は抑制した緑・黄・赤へ整理した。
- `ui/components.slint`へ`Z_LogicLCD`を追加し、Bars / Tempo / Signatureを数値用フォントで共通表示。メインのトランスポートHUDから直接利用するよう接続した。
- 固定幅の全面置換は未完了。次はInspector・Library・Editorの主要コンテナから`horizontal-stretch`を優先して適用し、1600px以外のウィンドウ幅で実機確認する。
- `Z_IconButton`へ`compact`モードと上辺ベゼルを追加。トラックヘッダーのM/S/Arm/Automationは24px幅の操作ボタンへ統一し、通常のラベルボタンと区別した。
- Arrangeのトラックヘッダーは選択色、トラックカラーバー、偶奇ゼブラ背景を併用する構成へ変更。大量トラック時の行追跡性を優先する。
- タイムラインのグリッド線を小節（#3A3A3C）、拍（#242426）、サブ拍（#202023）の階層表示へ変更し、リージョン背面で描画する構成に整理。
- Arrangeリージョンに12pxのヘッダー帯とクリップ名を追加。波形は中央基準のバイポーラ表示を維持し、ヘッダー上の可読性を優先する。
- `Z_Meter`は18px幅のセグメントLED表示とし、低レベルを青、通常域を緑、警告域を黄、クリップ域を赤へ分岐。
- `Z_Knob`へ正式な`compact`モードを追加。28pxノブ、1.5pxアーク、縮小した指標を使い、MixerのEQとPanへ適用した。旧来の`width: 22px`上書きによる内部描画のはみ出しも除去。
- `Z_SegmentButton`を追加し、トランスポートの先頭移動・再生・録音・ループを1つの結合セグメントとして表示。従来の独立ボタン間の大きな隙間を解消した。
- トラックヘッダーに28pxのトラックアイコン枠を追加し、既存のトラック`icon`値を表示。M/S/Arm/Automationは丸型compactボタンへ統一した。
- `Z_SegmentButton`は各ボタンの外枠を廃止し、親カプセルの外枠と内部の1px dividerだけで構成する方式へ変更。
- InspectorのTrackタブを2列化し、選択トラックと`Stereo Out`のcompactチャンネルストリップを横並びで表示。その他のInspectorタブは既存のスクロール編集領域を維持する。
- Inspector主要見出しを`Track Properties`、`Automation`、`Track Macros`、`Signal Chain`、`Spatial Audio`、`Effects Rack`などTitle Caseへ整理。
- トラックヘッダーへミニ音量バーとPan数値を追加し、音量操作は既存の`fader_changed` callbackへ接続。
- トップメニューバー左端へInspector（I）、Mixer（X）、Browser（B）のコンパクト切替ボタンを追加。主要パネルへキーボード以外の直接導線を用意した。
- サイクル領域の色を`Style.cycle`（Logic系イエロー）へ分離し、青系の選択色と区別。ルーラー上で再生範囲を認識しやすくした。
- MixerのAudio FXを縦型スロットへ整理。有効スロットはスレートブルー、空スロットは凹みのある暗色、電源状態とドロップダウン記号を表示する。
- Audio FXスロットを10個固定化し、プラグイン数に依存せずチャンネルストリップの高さと整列を維持。
- トップバー左端のInspector/Mixer/Browser切替を1つのセグメント枠へ統合。各ボタンの状態を同じグループ内で確認できるようにした。
- トラックヘッダーのPan表示を文字列だけの`P±値`からcompactノブへ変更し、`pan_changed` callbackへ接続。ミニ音量バーと合わせて直接操作できる構成にした。
- 主要な操作部からカラー絵文字を除去し、`SNAP`、`SCAN`、`CUE`、`DEL`など単色記号・短い機能名へ統一。OS標準フォントで色付き絵文字に置換される問題を避ける。
- Muteのアクティブ色を`Style.brand`（シアン系）へ変更し、Record Enableは`R`表記へ統一。Soloはアンバー、録音は赤の機能色を維持する。
- Inspector/Mixer/Browserのビュー切替グループをメニューバーからLCDと同じControl Bar段へ移動し、表示切替の階層を整理。
- `Z_SmartControls`を追加し、下部ビューのSmart Controlsから既存`track_macros` 8値を直接編集できるよう`macro_changed`へ接続。
- Audio EditorへFlex Time / Pitch表示モードを追加。既存波形上にトランジェントマーカーとモード表示を重ね、仮の音声解析値ではなく編集状態として切り替える構成にした。
- Piano RollへCC1 Modulation、CC11 Expression、CC64 Sustainの3つの編集レーンを追加。各レーンはドラッグで値を変更できるUI状態として実装。
- `Z_LiveLoops`を追加し、4×4セルの起動・停止グリッドを下部ビューへ接続。既存のStage/Launchpadとは独立したProduction導線を用意した。
- リージョン移動に4pxのドラッグ閾値を追加し、クリック選択と微小なマウス揺れによる誤移動を分離。
- オートメーションポイントは描画を10pxに保ちながら、TouchAreaを20pxへ拡大。精密なポイント操作のヒット率を改善した。
- Logic系テーマを再調整し、ベース（`#131315`）、パネル（`#232326`）、ヘッダー（`#2C2C30`）、サーフェス（`#1A1A1C`）、境界、LCD、機能色を共通`Style`へ集約。グリッド基準も48pxへ統一した。
- `Z_LogicLCD`を最小幅340px・高さ38pxへ拡張し、Position / Tempo / Signatureの3ブロック、固定幅数値、BPM suffixを表示する構成へ調整した。
- Mixerのフェーダーを通常220px・compact160pxへ拡張。暗いレーン、中央グルーヴ、24pxフェーダーキャップ、トラックカラーの2pxインジケーターを追加し、値の視認性を改善した。
- Slintで許可されない`y: 5%`の長さ指定を親高さに対する明示計算へ修正し、`cargo check --workspace`を再通過させた。
- Styleへ`highlight-top`、`shadow-bottom`、グリッド3階層、再生ヘッド、リージョン境界のトークンを追加し、個別色のハードコードを削減した。
- Arrangeの再生ヘッドに控えめなシャドウを追加し、ラインとヘッドを`Style.playhead`へ統一。トラック左端のカラータグは不透明度を下げ、長時間作業時の視覚的な圧を軽減した。
- ツール選択の絵文字・記号依存を減らし、`CUT` / `DRAW` / `M`の単色ラベルへ変更。グリッドとリージョン境界も共通Style経由へ移行した。
- フォント指定をApple専用名からSlint／OSの汎用ファミリー（`sans-serif` / `monospace`）へ変更し、Windows・Linuxでの欠落フォント置換を避ける構成にした。macOSのネイティブWindowタイトルバーは維持し、OS固有の最小化・最大化・閉じる操作をUIへ重複実装しない。
- Vulkan、Metal、Wayland、VST/CLAPのネイティブ埋め込みはまだ未接続。これらはUIコンポーネントから分離したplatform/backend層として別途実装する。
- UI全体の`font-size: 5px`〜`7px`を`Style.font-xxs`へ統一し、最小文字サイズを8px基準へ引き上げた。高度なADVパネルも同じ可読性基準で表示する。
- 実験的スキンのマゼンタ／原色青／強いオレンジを廃止し、`Style.brand`、`Style.ok`、`Style.solo`、`Style.record`へ集約。状態表示以外のアクセント色を増やさない。
- AppWindowの初期サイズを`preferred-width/height`へ移し、`min-width: 1080px`、`min-height: 680px`を追加。小型ディスプレイでの縮小余地を確保した。
- Spectrum、Piano Roll、Spectral Mapなどの大量Rectangle描画は現状Slintノードのまま。実波形・FFTのImage／GPUテクスチャ化と仮想スクロールは、データ供給層を分離してから実装する次段階とする。
- Productionモードでは`Z_HolographicSpec`と`Z_SpectralMaskingMap`を生成しないようにし、通常のMaster Monitor／Spectrumだけを表示する構成へ変更。重い解析HUDはADVモードへ隔離した。
- Piano Rollの鍵盤列をVerticalLayoutの積み上げからスクロール位置基準の絶対配置へ変更。表示範囲外の鍵盤は`visible`を下げ、スクロール中の不要な描画を抑制した。MIDIノート座標と編集操作は従来のピッチ基準を維持する。
- Piano Rollの128本の時間グリッドと84本のピッチグリッドにもViewport判定を追加。画面外の線を描画対象から外し、ズーム・スクロール時の再描画量を削減した。
- InspectorのSpectral Restorationをサンプル配列が空でも安全に表示できるよう、`sample_entries[0]`参照へ空状態ガードを追加した。
- AutomationのBezier Pathを点数2未満では生成しない条件付きコンテナへ変更し、空レーンや単一点レーンでの不正な`length - 1`依存を防いだ。
- サブエージェント監査で確認された次の未解決負荷を記録：タイムライン全件生成、Mixer全チャンネル生成、Piano Rollノート二重生成、CCイベント全件生成、再生ヘッド複製、`ui_timer`の広域伝播。次段階ではバックエンドから可視範囲モデルを供給する。
- `side_tab == 2`のProject画面が通常の2タブUIから到達不能だったため、Inspector / Library / Projectの3タブへ統一。Project Statusへの通常導線を追加した。
- Inspector幅を`Style.inspector-w`へ移し、固定値をデザイントークンで変更できるようにした。
- `industrial_matrix.slint`と`industrial_widgets.slint`は現在の`build.rs`から直接ビルドされていない旧UI系統。削除・統合は別変更として扱い、本番Styleと混在させない。
- `src/core/io/audio_driver_facade.hpp`の`DriverProtocol`二重定義を統合し、`#pragma once`を追加。単独include時のC++コンパイル破綻を解消した。
- `src/io/drivers/audio_driver_factory.hpp`の未定義だった`DriverFactory::create()`を実装。現状未接続のOS別バックエンドを成功扱いせず、明示的な`SilentDriver`へ安全にフォールバックする。実デバイス接続は別のplatform adapter実装が必要。
- VST3／CLAPホストの動的ライブラリ読み込みをOS分岐化。Windowsでは`LoadLibraryA`／`GetProcAddress`／`FreeLibrary`、macOS・Linuxでは`dlopen`／`dlsym`／`dlclose`を使うため、Windowsコンパイル時の`dlfcn.h`依存を除去した。Plugin instance生成とprocess接続は未実装のまま明示する。

## サブエージェント横断監査（2026-08-11）

### P0：UIモノリス

- `aura-ui/ui/aura_studio.slint`は約4,500行・50以上のコンポーネントで、AppWindowへ状態、レイアウト、操作、実験パネルが集中している。
- 循環importは確認されなかったが、`style → components/panels → aura_studio`の一方向依存に対して、AppWindowへの依存が過密になっている。
- 次の分割単位は`transport.slint`、`timeline.slint`、`mixer.slint`、`piano_roll.slint`、`browser.slint`、`overlays.slint`とし、AppWindowは接続だけにする。

### P1：操作部品と状態の不統一

- 共通`Z_IconButton`と個別`TouchArea`が混在し、フォーカス、キーボード操作、無効状態、アクセシブルな名前が統一されていない。
- `DAW_Actions`に多数のcallbackが集中している。Transport、Track、Timeline、MIDI、Automation、Plugin単位へ分割する必要がある。
- Mute／Solo／Recordが色だけで判別される箇所が残るため、文字・フォーカスリング・キーボード操作を追加する。

### P1：空状態・境界値

- サンプル一覧の`sample_entries[0]`直接参照、空のautomation lane、点数不足のPath描画を全てガードする必要がある。
- `editor_ph`の固定初期値、固定サンプルカタログ、固定CPU／サンプルレート表示がUIに残る。開発用モックと実測値を分離し、データ未接続時は`Unavailable`を表示する。

### P0：実装停止点

- `src/graphics/platform/vulkan_kernel.cpp`はInstance／Device初期化までで、Swapchain、CommandBuffer、Submit、Present、描画関数の多くが空実装。
- `src/core/plugins/vst3_host_processor.hpp`と`clap_host_processor.hpp`は動的ライブラリとエントリポイント解決までで、Plugin instance生成・process接続・GUIホストが未実装。現状は安全なバイパスであり、プラグインが鳴る状態ではない。
- `src/rendering/metal/waveform_compute.hpp`は実行時コンパイルを含むが、同期GPU処理と毎回のBuffer確保が残る。大量ファイルのバックグラウンド処理には専用キューと再利用バッファが必要。
- `src/io/audio_interface.hpp`には`DummyHardware`互換名が残る。実機オーディオをプレビュー合格条件にする場合、既定DriverがDummyへ落ちていないことを起動時に明示する。

### 優先順位

1. 空配列・固定モック表示のガードとUnavailable状態
2. `aura_studio.slint`のTransport／Timeline／Mixer分割
3. 共通FocusableControlとcallbackドメイン分割
4. Vulkanは未対応時のフォールバックを明示した上でSwapchain描画を実装
5. VST3／CLAPはinstance生成とprocess接続を実装するまで「対応済み」と表示しない
## 音声デバイス経路の整理（2026-08-11）

- `src/core/driver/mac_audio_driver_host.mm` が実際のmacOS CoreAudio HAL経路として、デバイス設定取得・再接続監視・オーディオコールバックを担当する。
- 旧 `src/io/coreaudio_driver.cpp` の `CoreAudioDriver` は初期化・開始が no-op で、ハードウェアを開いていないのに成功を返していたため削除し、`HardwareFactory::createDefault()` は明示的な `DummyAudioDriver` を返すよう変更した。
- 旧API利用者が実機出力を期待しないよう、実機経路と無音フォールバックを名前・状態で分離する。実機切替は `MacAudioDriverHost` の一本に集約する。
- `src/platform/audio_device.hpp` のOS抽象化は将来のWindows/Linux実装用として保持する。現状、非macOSは `SilentAudioDevice` で起動可能だが、WASAPI/PipeWire実装は未完了。

## 今回のUI保守改善（2026-08-11）

- `aura_studio.slint` に残っていた `Courier New` の直書きを `Style.numeric-font` に統一し、数値表示のフォントテーマを一箇所で変更できるようにした。

## 外部プラグインのロード状態（2026-08-11）

- VST3/CLAP は現在、共有ライブラリとエントリポイントの解決までで、DSPインスタンス生成・パラメータ列挙・オーディオ処理・GUI埋め込みは未実装。
- その状態で `loadVst3()` / `loadClap()` が `true` を返していたため、バイパス処理を「使用可能なプラグイン」と誤認できた。両メソッドは `LibraryResolved` と診断メッセージを保持しつつ `false` を返すよう修正した。

## サンドボックス境界の安全化（2026-08-11）

- 旧 `PluginSandboxHost` はSIGSEGV/SIGFPE/SIGILLをプロセス内シグナルハンドラと `siglongjmp` で回復しようとしていた。これはC++オブジェクト、ロック、アロケータ、オーディオスレッドの状態を壊し得るため撤去した。
- 現在のラッパーはC++例外だけを捕捉し、失敗後は無音バイパスへ移行する。ネイティブクラッシュからの隔離は同一プロセスでは実現できないため、VST3/CLAPの本格サンドボックスは子プロセスIPCとして別実装する。

## オフラインバウンスの入力検証（2026-08-11）

- `AudioExportEngine::bounce()` に、空パス・非有限/範囲外サンプルレート・未対応ビット深度・逆転/空のサンプル範囲の拒否を追加した。
- Classic RIFF/WAVの32-bitサイズ上限を超える出力を事前に拒否し、ヘッダー・サンプル書き込み・flushの失敗を成功扱いしないようにした。
- 旧 `TimelineSystem::render()` という存在しないAPI呼び出しを廃止し、バウンス時は `TimelineSystem::getTracks()` から各 `Track::processInto()` を通して実際のトラック信号を加算するよう接続した。

## TimelineSystemの空実装解除（2026-08-11）

- `addTrack()` が委譲コメントだけでトラックを保存していなかったため、null拒否・mutex保護付きで `m_tracks` へ登録する実装に変更した。
- `addMarker()` も現在のプレイヘッド位置と名称を `m_markers` へ保存するようにした。空名称は無視する。

## DSPリアルタイム安全化（2026-08-11）

- MIDIアルペジエーターのheld-note配列をMIDI全音域（128音）分だけ事前確保し、ノート追加時のヒープ再allocationを防止した。
- BPM・サンプルレート・サンプル間隔の有限値検証を追加し、BPM 0/負数/NaNによる除算・無限ループを拒否するようにした。
- ステップ番号を64-bit化し、長時間再生時の32-bitカウンタ巻き戻りを防止した。

## フィルターDSPの数値安定化（2026-08-11）

- ZDF/SVFのカットオフを5Hz〜Nyquistの49%へクランプし、低サンプルレートやNyquist直前の `tan()` 発散を防止した。
- レゾナンス/Q、不正なサンプルレート、係数のNaN/Infを検証し、異常時は安全な係数へフォールバックするようにした。
- nullバッファのブロック処理を早期returnし、DSP API境界での不正ポインタ参照を防止した。
- ステレオNEON SVFにも同じ入力・サンプルレート・カットオフ・Q・係数・出力の有限値ガードを追加した。

## 追加サブエージェント監査（2026-08-11）

### プレビュー前に残るP0

- `src/io/drivers/audio_driver_factory.hpp` と `src/io/audio_drivers.hpp` の汎用Factoryは、現在も明示的な無音フォールバックを返す。実機出力はmacOSの`MacAudioDriverHost`に限定され、Windows/LinuxのWASAPI・ASIO・PipeWireは未接続。
- `src/core/plugins/vst3_host_processor.hpp`、`clap_host_processor.hpp`はライブラリのエントリポイント解決までで、インスタンス生成・activate・process・GUI埋め込みは未実装。UIでは必ず「未対応」と表示する。
- `src/rendering/vulkan/vulkan_context.hpp` と `src/graphics/platform/vulkan_kernel.cpp`は、Swapchain、CommandBuffer、Submit/Presentまで実装済み。RenderPass／graphics pipeline／UI実描画は未実装のため、Vulkanを既定選択せずMetal/CPUへフォールバックする。
- `src/dsp/effects/true_peak_limiter.hpp`、`elastic_warp_engine.hpp`、`spectral_ducker.hpp`、`phase_vocoder.hpp`に、no-opまたはオーディオ処理中の確保が残る。マスター安全装置とタイムストレッチをプレビューの必須機能に含める場合は別途実装が必要。
- `src/core/engine/plugin_sandbox.hpp`、Rustの`audio_engine.rs`/`step_sequencer.rs`には、RTパス上のmutex・Vec生成・map操作が残る。再生コールバックから切り離し、prepare時確保＋SPSCイベント転送へ移行する。

### 今回の追加修正

- `AnalysisEngine`の相対ゲート履歴を固定1000要素リングバッファへ変更し、オーディオ処理中の`vector::push_back`と`erase(begin())`を除去。
- `SpectrumAnalyzer`と`AnalysisEngine`にnull入力、空ブロック、非有限サンプルレートの早期returnを追加。
- Rustの`LoudnessAnalyzerEngine`で左右入力の短い方に処理長を合わせ、NaN/Inf入力を無音化して範囲外アクセスを防止。
- Rustの`TruePeakLimiterEngine`で左右長、閾値、ceiling、サンプルレートを検証し、非有限入力を無音化。
- Rustの`AtmosEQ`でQを有限範囲へ制限し、フィルター出力がNaN/Infになった場合は状態をリセットして無音へ復帰。
- Vulkan未実装時に`initialize()`が成功を返さないよう変更し、既定GraphicsBackendからVulkanを除外。
- C++ `DelayLine::process()` のRust委譲コメントだけのパススルーを固定リングバッファ処理へ置換。遅延サンプルをクランプし、非有限入力を無音化。
- C++ `TruePeakLimiter` のno-opを解除し、有限値検証、簡易インターサンプル推定、ルックアヘッド遅延、ゲイン追従、ceiling適用を実装。
- C++ `ElasticWarpEngine::processWarp()` のno-opを解除し、ブロック間のソース位置を保持する固定・線形補間ベースのタイムスケール処理を実装。WSOLA/位相整合は今後の高度化対象として明示的に残した。
- `SidechainSpectralDucker`のFFTワークスペースをメンバーとして事前確保し、`analyzeAndDuck()`内の`vector<complex>`生成を除去。Contextの正式な`sidechainBuffer` APIへ接続し、空入力・不正サンプルレート・非有限amountも検証。
- `PhaseVocoder`の再帰FFTにあった毎回の`even`/`odd`確保を除去し、固定ワークスペースと反復型radix-2 FFTへ変更。入力・出力長とratioの有限値も検証した。
- `AtmosPanner`のITD計算に残っていた`44100.0`固定値をインスタンスのサンプルレートへ変更。48/96kHz向けの更新API、チャンネル数・null入力ガードも追加した。
- Rust `AudioOrchestrator::process_graph()` のノード`collect`と毎回の1024サンプルVec生成を除去し、事前確保したスクラッチバッファをswapして再利用する構造へ変更。なお、この関数には依然として各DSP Mutexがあるため、ハードウェアコールバックへ直接接続する前にSPSC/スナップショット化が必要。
- Rust `StepSequencer`にcaller-ownedの`process_tick_into()`を追加し、イベントVecをブロック間で再利用できるようにした。1ステップあたりのイベント数を128件に制限し、空パターンの剰余演算も防止。既存のVec返却APIは互換性のため残し、RT接続時は新APIを使用する。
- C++ `PluginSandbox`に明示的な`registerPlugin()`を追加し、`safeProcess()`/安定性更新で未知IDを`unordered_map::operator[]`により暗黙挿入しないよう変更。なお、Mutex・計時・同一プロセス例外捕捉は残るため、真のRTサンドボックスは子プロセスIPCが必要。
- `StepSequencer::process_tick_into()`を複数ステップ境界に対応させ、大きなホストブロックでもイベントを1ステップだけ取りこぼさないよう修正。各イベントにブロック内のサンプル位置を付与し、長時間タイミングの再現性を改善。
- 実際に公開されている`SequencerOrchestrator`にも`generate_events_into()`を追加し、caller-owned Vecの再利用を可能にした。既存の`generate_events()`は互換性のため維持し、テストを再利用API経由へ変更。
- `AudioEngine::analyze_spectrum()`のサンプル位置ベースの疑似64バンド解析を、実周波数のGoertzel解析へ置換。非有限入力を無音化し、spectral centroidをバンド番号ではなくHzで返すようにした。
- `AudioEngine`のサンプルレートを8kHz〜384kHzへ検証し、EQゲイン、マスターリミッター閾値、3D座標、入力サンプルのNaN/Infを安全な値へ正規化。
- 旧`DriverFactory::SilentDriver`に初期化検証、running状態、callback保持、`isSilentFallback()`を追加。無音ドライバを実デバイスと混同しない状態照会を可能にした。
- `IProcessor`に不足していた`<string>`/`<vector>` includeを追加し、未実装パラメータを自動化済みと誤報していた既定`isParameterAutomated()`を`false`へ変更。
- `AudioExportEngine::writeSamples()`でNaN/Infサンプルを無音化し、バッファ長・チャンネル数を検証。24-bit PCMはホストエンディアン依存のポインタ書き込みをやめ、明示的なlittle-endian 3バイトへ変更。
- 公開Rust版`elastic_warp_engine.rs`の実処理にも、低サンプルレート時のgrain/window境界、左右入力長不一致、NaN ratio、2の累乗でないリングマスクによる循環破綻を修正。OLA容量を2の累乗へ統一し、空入力は無音で終了する。

### 合格条件の再確認

`cargo test --workspace` は67件すべて成功。これはコンパイルとユニットテストの合格であり、実機CoreAudio再生、デバイス切断、GPU描画、外部プラグイン動作の合格を意味しない。プレビュー公開前にはmacOS実機で再生・停止・保存・読込・バウンスを手動確認する。

## 追加の横断修正（2026-08-11）

- `AtmosReverbEngine`で12ch未満・チャンネル長不一致・NaN/Infパラメータを拒否または安全値へ正規化。フィードバックとダンピングを安定範囲へ制限し、出力の非有限値を無音化。監査関数も常に`true`を返す実装から、遅延線・インデックス・状態を検査する実装へ変更した。
- `DeEsserEngine`の検出フィルターをNyquist未満へクランプし、8kHzなど低サンプルレートでも不正な高域係数を生成しないようにした。ゲート処理は左右バッファの短い方に処理長を合わせ、短いサイドチェーン、NaN/Inf入力、ゲイン状態を安全に処理する。
- `TempoOrchestrator`にイベント正規化を追加。外部編集で混入した不正BPM、未整列イベント、同一サンプル位置の重複を解消し、サンプル位置0の基準イベントを保証してから二分探索を行う。`audit_tempo()`も常に成功するスタブを廃止した。
- 上記の境界条件を再現するユニットテストを追加し、`cargo test --workspace --no-fail-fast`は69件すべて成功。

### まだ残るP0/P1

- 実機オーディオ出力はmacOS CoreAudio経路に限定され、Windows WASAPI/ASIO・Linux PipeWire/JACKは未接続。SilentDriverは起動用フォールバックであり、音声出力の代替実装ではない。
- VST3/CLAPはエントリポイント解決までで、インスタンス生成、activate/process、パラメータ、GUI埋め込み、子プロセスIPCサンドボックスは未実装。
- Vulkanは初期化失敗を正しく報告し、Swapchain、CommandBuffer、Submit/Presentまで実装済み。RenderPass／graphics pipeline／実描画は未実装のため、MetalまたはCPUバックエンドを既定にする。

### 2026-08-12 Vulkan/macOS追加対応

- `aura-core-bridge/build.rs` に `AURA_ENABLE_VULKAN=1` の明示的オプトインを追加した。通常ビルドへVulkan SDK／MoltenVKのリンクを強制せず、環境が揃った場合だけVulkan translation unitと`vulkan`ライブラリを有効化する。
- `src/graphics/platform/vulkan_kernel.cpp` は、Vulkan instance、physical device、graphics queue family、logical deviceの実在確認まで行う。Surface／Swapchain／CommandBuffer／Submit／Present／shader pipelineが未接続のため、ここで成功扱いせず、MetalまたはCPUへフォールバックする。
- Vulkan instance作成時は実行環境の対応APIバージョンを取得し、1.3固定要求で古いloader／MoltenVKを不必要に拒否しない。
- `src/rendering/vulkan/vulkan_context.hpp` は、外部から渡されたSurfaceに対してgraphics+present queue familyを選択し、logical deviceとqueueを所有する。さらにSurface capability／format／extentに基づくSwapchain、image view、command pool、command buffer、semaphore／fenceを作成できる。
- 同ファイルに最小のAcquire→画像clear command→Submit→Presentフローを追加した。RenderPass／graphics pipeline／UIの実描画は引き続き未実装で、Vulkanはクリアフレーム検証と描画基盤の段階である。
- `aura-core-bridge/build.rs` はVulkan有効ビルド時に`glslc`が存在すれば`vulkan_ui.vert`／`vulkan_ui.frag`をVulkan 1.2向けSPIR-Vへ検証コンパイルする。SPIR-Vの組み込みとGraphics Pipeline接続は引き続き未実装。
- 同ビルド処理は生成SPIR-Vを`aura_vulkan_ui_spv.hpp`へ変換し、実行バイナリへ埋め込む。`VulkanContext`はDevice初期化時にVertex／Fragmentの`VkShaderModule`を生成し、Swapchain作成時にRenderPass、Framebuffer、Descriptor、固定長1024矩形のUniform Buffer、インスタンスGraphics Pipelineまで生成・破棄できる。`VulkanGraphicsKernel::initializeWithSurface`から外部Surfaceを渡し、フレーム内の矩形群を1回の`vkCmdDraw`でPresentまで実行できる。macOS向けには`vulkan_surface_macos.mm`でCAMetalLayer→`VK_EXT_metal_surface`変換を追加し、macOSのKernel初期化時に必要なInstance拡張を検証して自動接続する。Windows向けWin32 HWND、Linux向けX11 Display/WindowのSurface factoryも追加したが、Windows/LinuxのInstance拡張有効化と自動Kernel接続、Wayland対応、大量UI以外の描画プリミティブは引き続き未実装。
- `src/core/driver/mac_audio_driver_host.mm` はCoreAudio再接続に失敗した場合、旧デバイス設定の入力キューを破棄してから次の録音セッションを開始する。デバイス切断後の古い入力ブロック混入を防止する。
- バウンスの実レンダリングと外部デバイス切断時の再接続・自動Pauseは、macOS実機での手動検証が必要。

## 追加の境界安全化（2026-08-11）

- Rust `DelayLineEngine`で空バッファ、破損した公開write index、NaN/Infサンプルを安全に処理し、遅延線の監査で容量・マスク・インデックス・内容を検証するようにした。
- Rust `MetronomeOrchestrator`でBPM 0/負数/NaN、無効なサンプルレートを無音で拒否し、既存のNaN/Inf出力を汚染しないようにした。監査もサンプルレートを検証する。
- Rust `DynamicsOrchestrator`の解析入力でNaN/Infを無音扱いにし、crest factorとノイズフロア推定が異常値に汚染されないようにした。
- 境界条件テストを追加し、`cargo test --workspace --no-fail-fast`は71件すべて成功。`cargo check --workspace`も直前のバッチで成功済み。

## 解析・サンプラーの追加修正（2026-08-11）

- `TempoAnalyzerEngine`でサンプルレート範囲を検証し、NaN/Inf入力を無音扱いにした。BPM自己相関のlagを最低1サンプルへ制限し、異常なconfidenceを返さないようにした。
- `VocalSynthKernel::audit_vocal_synth()`でNaN/Infおよび低すぎるサンプルレートを不正状態として検出するようにした。
- `SamplerEngineEngine`で左右出力長の短い方に処理を合わせ、出力のNaN/Infと過大値を制限。監査でサンプルレート、64ボイス、ボイス状態、音源チャンネル長、ループ境界を検証するようにした。
- 解析系の異常入力テストを追加し、`cargo test --workspace --no-fail-fast`は72件すべて成功。

## C++オーディオ境界とプレビューサンプル寿命（2026-08-11）

- `AuraUnifiedEngine::processBlock()`で空ブロック、2ch未満、範囲外offsetを早期拒否し、要求サイズをバッファ末尾へクランプ。`processBlockDirect()`でもnullポインタ、1ch以下、空ブロックを無音化してから終了するようにした。
- プレビューサンプルの再登録・消去時、再生停止中かつコールバックが quiescent の場合に旧スナップショットを回収するようにした。再生中はRTスレッドの生ポインタを壊さないため旧データを保持する。
- `aura_unified_engine.cpp`のClang構文検証に成功し、Rustワークスペースは72テスト全件成功。

## 実機オーディオ境界の追加確認（2026-08-11）

- `MacAudioDriverHost`は既にデバイス設定を取得してから`prepareToPlay()`し、その後にCoreAudio開始する順序になっていることを確認。デバイス変更通知は再接続フラグへ渡り、UIから`try_reconnect_audio_device()`を呼べる。
- HALコールバックは非インターリーブ2ch以外を音声破壊せず無音化し、停止中・nullデータ・未実行状態も無音で返す設計を維持した。
- 実機確認で残る項目は、ヘッドフォン抜去時の自動Pause、再接続後のサンプルレート変更、実際の出力ピークとcallback counterのUI反映。これは静的チェックだけでは合格にできない。

## AudioBuffer入力境界の追加修正（2026-08-11）

- `AudioBuffer::wrapChannels()`でチャンネル配列内のnullポインタを検出し、外部バッファとして登録しないようにした。
- `getReadPointer()`/`getWritePointer()`にチャンネル範囲チェックを追加し、offset付き取得もnullとバッファ末尾を安全に扱うようにした。
- `aura_unified_engine.cpp`と`audio_buffer.hpp`のClang構文検証に成功。実機CoreAudioの自動Pauseは既存のUIテレメトリ経路で継続確認する。

## MIDI・クロック状態の追加安全化（2026-08-11）

- `ArpeggiatorEngine`でBPM/サンプルレート不正値、ブロック終端のu64オーバーフローを防止し、監査でheld note、step index、active note、乱数状態を検証するようにした。
- `EngineClockOrchestrator`で無効なnominal/effective rateやNaN状態による再生位置破綻を防止し、MIDI tick計算のBPM・resolutionも検証するようにした。
- `MPEOrchestrator`でpressure/timbre/pitch bendを正規化し、無効なbend rangeを拒否。note-to-channel対応と全voice状態を監査するようにした。
- 公開Rust版`DeEsserEngine`で左右バッファの短い方に処理長を合わせ、NaN/Inf入力・過大出力を防止し、状態監査を実装した。
- `cargo test --workspace --no-fail-fast`は72件すべて成功。

## マスター・テープ・フェイザー処理の追加安定化（2026-08-11）

- `MasterLimitProEngine`と`AnalogClonerEngine`で左右長不一致、NaN/Inf入力、異常なceiling/drive、過大出力を防止した。
- `MetronomeOrchestrator`の出力を有限値・安全範囲へ制限した。
- `StereoPhaserEngine`で無効サンプルレート、rate/mix/feedback異常、左右長不一致、再帰状態の非有限化を防止し、状態監査を追加した。
- `TapeMachineEngine`で無効サンプルレート・drive/noiseとNaN入力を正規化し、左右長不一致と過大出力を防止。遅延線も監査する。
- `cargo test --workspace --no-fail-fast`は72件すべて成功。

## フィルター・メトロノーム・ボーカル処理の追加安定化（2026-08-11）

- `StateVariableFilterEngine`でサンプルレート、カットオフ、Q、ゲインを安全範囲へ制限し、係数・状態・出力のNaN/Inf発散時に状態をリセットするようにした。左右バッファ長も短い方へ合わせた。
- `StepFilterEngine`で左右長不一致とplayhead加算のオーバーフローを防止し、内部係数・16ステップ値を監査するようにした。
- `ConsoleModelEngine`で左右長不一致、NaN入力、異常なdrive/EQ状態を防止。
- `VocalPitchCorrectorEngine`で外部から壊れたwindow sizeを受けても読み出し範囲を越えないようにした。
- `cargo test --workspace --no-fail-fast`は72件すべて成功。

## チューナー・サンプラー・ラウドネスの追加修正（2026-08-11）

- `VocalPitchCorrectorEngine`の自己相関lag上限をバッファ内へ制限し、最大lagの隣接参照による範囲外アクセスを防止。左右長、SR、amount/speed、NaN入力も安全化した。
- `AuraSamplerAdvancedEngine`で外部から破損したlayer indexを無音終了し、NaN/Infサンプルを無音扱いにした。layer、velocity、再生位置を監査する。
- `LoudnessAnalyzerEngine`のリングバッファ、窓長、累積値、最新メトリクスを監査するようにした。
- `StereoImagerEngine`で左右長不一致、無効SR、NaN入力、過大出力を防止し、状態監査を実装した。
- `cargo test --workspace --no-fail-fast`は72件すべて成功。

## リバーブ・ディレイ系の追加安定化（2026-08-11）

- `ReverbCoreEngine`で無効なサンプルレート、TPT係数の不正値、T60不正値、左右長不一致を防止。遅延線・read位置・ゲイン・フィルター状態の監査を実装した。
- `DivineReverbEngine`のAllPass遅延長を最低1へ保証し、左右バッファ長の不一致と過大出力を防止。
- `ReverseDelayEngine`で無効なwindow時間、空バッファ、window超過を拒否し、mixと出力を安全化。
- `PingPongDelayEngine`でBPM 0/NaN、無効sample rate/note value、左右長不一致を防止し、遅延線状態を監査。
- `cargo test --workspace --no-fail-fast`は72件すべて成功。

## ステレオDSP入力長とサイドチェーンの追加修正（2026-08-11）

- `StereoTremoloEngine`、`SidechainDuckEngine`、`SidechainCompressorEngine`、`StereoChorusEngine`で左右バッファ長の短い方に処理を揃え、NaN/Inf入力・不正BPM/SR・不正パラメータによる除算や出力破綻を防止した。
- サイドチェーンバッファが短い場合は不足サンプルを無音扱いにし、圧縮率・ゲイン・ミックスを安全範囲へ制限した。
- 各監査関数を、サンプルレート、状態、係数、遅延線の実状態を検証する実装へ変更した。
- `cargo test --workspace --no-fail-fast`は72件すべて成功。

## 波形編集・FDN境界バグの追加修正（2026-08-11）

- `SlicingOrchestrator`の空/短い波形での`len - 1`アンダーフローを防止し、NaN/Infサンプルを無音扱いにした。窓インデックスをサンプル位置へ正しく変換し、ゼロクロス探索範囲も安全化した。
- `ZeroCrossingOrchestrator`で非有限サンプルを安全に扱い、空データ時の境界を維持した。
- `VirtuosoSpaceEngine`で低サンプルレート時の遅延線長を最低1へ保証し、左右入力長不一致、無効な状態、過大出力を防止。監査関数を実状態検証へ変更した。
- `cargo test --workspace --no-fail-fast`は72件すべて成功。
### 2026-08-12 RT安全性・PDC接続追加対応

- `src/core/effect_chain.hpp` のProcessorList世代回収を、音声スレッドのReaderGuardで保護した。公開ポインタ交換後に既存読者が残っている場合は古いリストを保持し、Use-after-freeを避ける。
- `src/core/engine/pdc_manager.hpp` を `AuraUnifiedEngine::syncStructuralChanges()` に接続した。RoutingEngineの接続と各Track／BusのEffectChain遅延を制御スレッドで再計算し、Track側は事前確保リング遅延（最大8192サンプル）で補償する。
- 外部VST3／CLAP／AUの別プロセスサンドボックスは未完了。現状はインスタンス生成・process・IPCヘルパーが存在しないため、外部形式を成功扱いせず安全に拒否する。次段階では専用の`aura-plugin-host`ヘルパー実行ファイルと共有メモリ／ウォッチドッグを追加する。
- `src/core/plugins/plugin_sandbox_host.hpp` に制御スレッド専用のヘルパー起動・停止・`pollHealth()`を追加した。`AURA_PLUGIN_HOST_BIN` が未設定なら起動せず、音声スレッドの`isAlive()`は原子値のみを読む。共有メモリで音声を往復する処理とWindows Job Objectはまだ未接続であり、処理済み音声を返すとは扱わない。
- `src/core/engine/tempo_map.hpp` の`setBPM()`を現在のサンプルレートへ追従させ、ランプ区間の`getBPMAt()`を線形補間へ修正した。テンポマップ未接続のUI側固定BPM変換と、ランプ／サンプルレート変更の統合テストは引き続き必要。
- `AuraUnifiedEngine`／CXX bridge／`aura-ui/src/slint_ui.rs`へ`samples_to_beats`／`beats_to_samples`を接続した。MIDIノート、リージョン表示・移動・分割、スクラブ、再生ヘッド表示は固定BPM式ではなくCoreの変換APIを使う。複数テンポイベントをUI操作から編集するAPIと実テンポマップ編集UIは未実装。
- `TempoMap`にテンポイベントの追加・削除・クリアを追加し、CXX bridgeと`DAW_Actions`の`set_tempo_event`／`remove_tempo_event`へ接続した。ランプ区間のサンプル→ビート積分は正確な二次式、逆変換は有界Newton反復へ変更した。イベント一覧を表示・ドラッグ編集する専用UIは未実装。
- `Z_GlobalTempoEditor`に現在の再生位置をイベント位置として表示し、`APPLY`／`REMOVE`操作を追加した。現段階は再生ヘッド位置への単一イベント編集導線で、複数イベント一覧・ドラッグ移動・Undo統合は未実装。
- オーディオデバイス未接続時はUIポーリングで再生を自動Pauseし、録音状態も停止して不完全な入力を誤って確定しないようにした。再接続・サンプルレート変更・実機抜去時の長時間挙動は、実CoreAudioデバイスでの確認が必要。
- `AudioEngine::is_audio_device_ready()`にも同じ防波堤を追加し、UIを経由しないBridge/ホスト呼び出しでもデバイス切断時にCore再生を停止する。録音プレビューの確定は引き続きUIの明示操作または切断処理でのみ行う。
- `DAW_Actions.set_snap`を`AppWindow.snap_division`／Piano Rollへ接続し、1/1〜1/64（3連符を含む）の選択が表示だけでなくノートグリッド計算にも反映されるようにした。
- トラックのPhase InvertをUI状態だけのトグルから、atomic状態を持つTrackの実オーディオ反転処理へ接続した。Core/FFI経由で失敗もUIへ返す。
- Phase Invertをプロジェクト形式v15へ保存し、Open時の復元とトラック複製時の継承にも対応した。
- `ChannelStrip`のゲイン／Pan平滑化をブロック単位からサンプル単位へ修正。再生中のフェーダー・Pan操作でブロック境界の段差を作らない。
- 内蔵プラグイン画面の`set_filter`を選択トラックの実IDとFX 0へ接続し、LimiterパラメータスライダーがUIだけでなく実Processorへ届くようにした。外部VST3/CLAPは引き続き未接続時に明示拒否する。
- `TempoMap::getEvents()`、CXX bridge、`AuraCore::get_tempo_events()`を追加し、`Z_GlobalTempoEditor`へ複数イベントの一覧・選択・再編集・削除・ドラッグ移動を接続した。停止中も一覧を同期する。テンポ追加・上書き・削除・移動は`UndoTransactionManager`へ接続済み。
