# SKILL: Hirari Offline Rendering (Bounce Engine)

## 💽 Export & Bouncing Directives
ユーザーが楽曲を最終的にWAVファイル等に書き出す「オフライン・レンダリング」の掟です。

- **Non-Realtime Speed**: 書き出し（バウンス）処理は `TimelineSystem` を最速（As Fast As Possible）の `while` ループで回し切ってください。リアルタイムの `sleep_for` やUI待ちを挟んでストールさせてはいけません。
- **Block Size**: リアルタイム再生時は音の遅延を防ぐためブロックサイズを 256 や 512 にしますが、オフラインレンダリング時はプロセッサキャッシュを最大限活用するため、ブロックサイズを 4096 や 8192 に拡張して一気に計算・書き出すこと。
- **Final Dither (重要)**: 24-bit WAV や 16-bit WAVへの最終書き出し時のみ、`MasterSuite` の一番最後で TPDFディザー を発動させること。（32-bit Float書き出し時はディザーをバイパスしてください）。
