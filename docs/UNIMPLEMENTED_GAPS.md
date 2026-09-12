# Aura 現行ギャップ一覧

このページは、現在の公開版に残る未接続・未検証項目だけを扱う正本です。
過去の監査結果は [`archive/UNIMPLEMENTED_GAPS_2026-08-11.md`](archive/UNIMPLEMENTED_GAPS_2026-08-11.md) に保存しています。

## 外部環境に依存する項目

- 実オーディオデバイスの全機種マトリクス検証
- Windows ASIOバックエンドの実機検証
- 第三者VST3/AU/CLAPの網羅的互換性
- ネイティブプラグインGUI埋め込み
- ARA2パートナープラグイン連携
- Dolby Atmos等の外部レンダラー連携

## 製品化前に必要な確認

- 長時間・大規模プロジェクトの安定性測定
- UIから全Core機能を操作する統合確認
- 実機I/Oを含む録音・再生・書き出しの回帰テスト

機械可読な判定は `engine_capabilities_json()` の `status` と
`verification` を参照してください。`implemented` はエンジン契約の実装を示しますが、
実機・第三者プラグインでの検証済みを意味しません。
