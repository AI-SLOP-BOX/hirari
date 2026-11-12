---
name: ameui-guide
description: AmeUI (AI-SLOP-FamilyのコアUIフレームワーク) の機能解説、何ができるのか、リクエスト/不具合報告の記載ルール、CLI機能一覧、および開発・検証フローを提供するガイドスキル。HirariやKagariなどファミリープロジェクトからAmeUIを利用・機能要望する際にも使用。
---

# AmeUI 総合利用・開発ガイド

AmeUI（`ame-ui`）は、AI-SLOP-Family（Hirari、Kagari、Moufu等）の中核を担う、Rust製の高パフォーマンス・クロスプラットフォーム宣言的GUIフレームワークです。

---

## 1. AmeUI は何ができるのか？（コア機能一覧）

### ① 高性能レンダリング＆次世代テキストエンジン
- **GPU レンダリング**: Vello（wgpu / WGSL）による2Dベクター描画。Retained display list による静的ブランチの差分再利用。
- **テキストパイプライン**: Parley 0.12 + HarfRust による高度なテキストシェーピング。Fontique によるゼロコピーフォント解析、ICU4X 準拠の CJK（日本語・中国語）禁則処理・双方向テキスト（BiDi）対応。
- **高DPI & ピクセルグリッドスナップ**: Retina / 4K / 8K ディスプレイでのフォント・ボーダーの完全なシャープ化。

### ② 豊富なプロフェッショナル向け標準ウィジェット
- **基本コントロール**: `Button` (Primary/Secondary/Destructive/Ghost/Icon), `TextField`, `TextArea` (IMEプレエディット・ソフトラップ・矩形選択・Undo/Redo対応), `Checkbox`, `Slider`, `Select`, `Stepper`, `Badge`, `ProgressBar`
- **クリエイティブ / プロ向けコントロール**:
  - `TimelineView`: 映像・DAW・シーケンサ向けルーラー＆トラック。秒単位 / BPM・拍子小節（Beat/Bar）軸、制御可能ズーム/パン、マーカー追加/削除、ヘッダーインデント対応。
  - `GradientPicker`: マルチストップ（2〜32ストップ）、角度、色補間対応。
  - `CurveEditor`: ベジェ/イージング曲線制御（端点固定・2〜128制御点）。
  - `ColorPicker`: SV矩形・色相スライダー・RGB/HEX入力。
  - `CommandPalette`: クエリ検索、最近使用履歴（32件優先ランキング）、ファジーマッチング。
  - `DiffView`: unified / side-by-side コード差分表示。
  - `FontPicker`: システムフォント列挙・プレビュー。
- **レイアウト・コンテナ**:
  - `VStack`, `HStack`, `ZStack`, `Grid`, `AdaptiveGrid`, `Wrap`, `Center`, `Frame`, `RelativeFrame`, `LayoutLimits`, `Padding`, `Grow`, `Aligned`
  - `ScrollView`, `HorizontalScrollView`, `SyncScroll`
  - `VirtualList`: 何万行ものデータを可視範囲の数ノード分のみメモリ展開・高速スクロール。
  - `SplitPane`: アニメーション付き可動分割ペイン。
  - `Overlay` / モーダルダイアログ（クリック外Dismiss、フォーカス復元）。

### ③ 宣言的 DSL `.ame` 言語 & 複数言語バインディング
- **`.ame` 言語**: 型安全な状態バインディング（`State<T>`）、リアクティブ式、アニメーション定義（`spring`, `ease_in_out` 等）。
- **マルチホスト対応**:
  - Rust Live API（ネイティブ）
  - JSON ViewTree プロトコル（JSON Schema v65/v69 準拠、`json_host`）
  - Python (`python/`), JavaScript / TypeScript (`javascript/`), Java (`java/`), Go (`go/`)

---

## 2. CLI ツール機能一覧 (`amec`, `amec-lsp`, `json_host`)

AmeUI にはアプリ作成・構文解析・コード生成・プレビュー・LSP連携を担う CLI ツール群が同梱されています。

### ① `amec`（Ame UI コンパイラ・開発CLI）
`.ame` 言語のビルド、検証、コード生成、アセット抽出、ネイティブプレビューを行う中心コマンドです。

```bash
cargo run --bin amec -- <COMMAND> [OPTIONS]
# またはインストール済みの場合: amec <COMMAND> [OPTIONS]
```

| サブコマンド | 構文 | 機能・説明 |
|---|---|---|
| `--version`, `-V` | `amec --version` | バージョン、Ame言語エディション、ViewTreeスキーマ版数、ホストプロトコル版数を表示。 |
| `check` | `amec check <FILE.ame>` | `.ame` ファイルの構文解析・型検査・診断を実行し、構文エラーがないか検証。 |
| `fmt` | `amec fmt <FILE.ame> [--check]` | `.ame` ソースコードを標準スタイルで自動フォーマット。`--check` で差分チェック（CI用）。 |
| `preview` | `amec preview <FILE.ame> [--watch] [--metrics] [--inspect]` | ネイティブGUIウィンドウで `.ame` アプリを即時プレビュー。<br>・`--watch`: ファイル保存時に自動ホットリロード<br>・`--metrics`: フレームレート・レンダリング負荷メトリクスを表示<br>・`--inspect`: レイアウトツリーインスペクターを有効化 |
| `run` | `amec run <FILE.ame>` | ネイティブGUIアプリとして単体実行。 |
| `tree` | `amec tree <FILE.ame> [--action <NAME> ...]` | コンパイル後の ViewTree を JSON 形式で stdout にダンプ（状態やアクション呼び出し後のツリー構造検証に有用）。 |
| `codegen` | `amec codegen <FILE.ame> --target typescript\|python` | `.ame` の公開インターフェースや状態バインディングから、TypeScript または Python の型定義コードを自動生成。 |
| `export-assets` | `amec export-assets <FILE.ame> [--output <DIR>]` | `.ame` 内に含まれるインラインSVGや画像アセットを抽出し、指定ディレクトリ（デフォルト `assets/`）へ一括書き出し。 |
| `new` | `amec new <NAME> --android [--output <DIR>]` | Android NativeActivity 用の AmeUI スタータープロジェクト（Gradle, AndroidManifest, NDK Rustバインディング含む）を自動スキャフォールド。 |

### ② `amec-lsp`（Language Server Protocol 実装）
VSCode や IDE 向けに `.ame` 言語のエディタ機能を提供するバックエンドです。
- JSON-RPC over stdin/stdout
- リアルタイムシンタックスエラー診断（Diagnostics）
- シンボル定義ジャンプ・ホバー情報・リファクタリング支援

### ③ `json_host`（言語非依存ヘッドレス/GUIホスト）
Python, TypeScript, Go 等の他言語から AmeUI を駆動するための標準プロトコルホストです。
- 標準入出力（stdio）経由で JSON ViewTree を受信してネイティブ描画
- マウス・キーボード・IMEイベントを JSON でホスト言語へストリームバック
- ヘッドレスモードでのスクリーンショット・幾何計算テストハーネスとして動作

---

## 3. リクエスト・不具合報告の配置場所と記載基準

AmeUI に対する機能要望、API追加、バグ報告は **リポジトリ内の `リクエスト.md`** に集約して管理しています。

### 📄 リクエストファイルの場所
- **正本（プライマリ）**: `/Users/dev/Desktop/AI-SLOP-Family/AmeUI/リクエスト.md`
- ※ AI-SLOP-Family ルート（`/Users/dev/Desktop/AI-SLOP-Family/リクエスト.md`）にも横断課題がありますが、AmeUI 単体の仕様・API・バグ修正は本リポジトリの `リクエスト.md` に記載します。

### ✍️ 優先度（P0〜P3）の判定基準

| 優先度 | 判定基準・適用条件 | 具体例 |
|:---:|---|---|
| **P0** (緊急/ブロッカー) | ・ファミリーアプリ（Hirari, Kagari等）の開発・実行を完全に止めている重大不具合<br>・CIのビルド破損、型破壊、パニック、全テスト失敗<br>・設計アーキタイプやプロ向け中核機能の完全な欠落 | ビルドが通らない、Timeline描画でパニック、ゼロショットプロUIの規約欠損 |
| **P1** (高優先) | ・次期リリースで必須の機能・API追加<br>・既存ウィジェットの操作感ギャップ（コールバック不足、IME入力不全、スクロール境界バグ）<br>・未対応の標準ウィジェットやDSLバインディングの充足 | スライダー確定時のみon_release、TextFieldのfocus/blur検知、TextOverflow省略 |
| **P2** (中優先) | ・ファイル分割・巨大モジュールのリファクタリング（800行以下ルール維持）<br>・パフォーマンス最適化（レイアウトキャッシュ、レンダリング再利用）<br>・CI高速化、一般的なUIユーティリティ追加 | `render.rs` や `parser.rs` の責務別分割、検索デバウンス、リスト差分更新 |
| **P3** (低優先/改善) | ・ドキュメント・サンプルの記述修正<br>・エラー診断メッセージの行番号精度向上<br>・dead_code 警告の解消、コードクリーンアップ | rustdocの誤字修正、サンプルの非推奨API更新、未使用importの整理 |

### 📝 記載ルール・フォーマット
`リクエスト.md` の「優先して進める残件」表に以下の形式で追記します：

```markdown
| 優先 | ID | 残っている作業 |
|---|---:|---|
| P1 | REQ-XXX | 【概要】具体的な要望内容（期待される挙動、対象プラットフォーム、関連するAPIなど）。残件と完了条件を明記。 |
```

#### ⚠️ 重要な運用ルール（過去の教訓より）
1. **Partial（部分対応）のまま ✅ にしない**:
   - 一部の機能が入っても、未充足の残件がある場合は「部分対応: ○○完了、残件: △△」と記載し、すべて完了するまで完了扱いにしない。
2. **「対応済み」の定義をテスト名に紐づける**:
   - 完了とする際は、必ずその動作を担保するテスト（ユニットテスト名や `tests/` 内の統合テスト名）を記載する。
3. **孤立ファイルへの追加は無効**:
   - `mod` に接続されていないテストファイル（例: 旧 `src/widgets/tests/rendering_controls.rs` 等）にテストを書いてもテストスイートで実行されない。必ずライブな実行経路（`src/widgets/tests.rs` や `tests/*.rs`）で検証すること。

---

## 4. 開発・検証フロー（Macストレージ保護・Linux CI連携）

AmeUI では Mac ローカルのストレージ消費（`target/` の肥大化）を防ぐため、**Linux ビルドサーバー（aislopbox@192.168.0.21）** を活用した高速ビルド・テスト運用を徹底しています。

### 🔄 基本の開発サイクル
1. **コード変更・コミット**:
   ローカルで不要な `cargo build` を走らせず、編集後にコミット。
2. **Linux リモートへプッシュして自動テスト**:
   ```bash
   git push linux main
   ```
3. **CI ログの確認**:
   ```bash
   ssh aislopbox@192.168.0.21 "tail -n 25 ~/ci-logs/ameui.log"
   ```
   - `mold` リンカーとインクリメンタルキャッシュにより、プッシュから20〜25秒で全280件以上のユニットテストが完了し、`UNIT TEST OK` が表示されます。
4. **ファイル行数ルール**:
   - 保守性と可読性向上のため、**「1ファイル最大800行程度」** を厳守してモジュール分割を実施。
