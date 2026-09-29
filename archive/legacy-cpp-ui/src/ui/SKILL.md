# SKILL: Hirari UI (User Interface Frontend)

## 🖥️ Threading & Safety Rules (大罪防止条約)
GUI側（画面やマウス操作）を作る際の絶対的なルールです。オーディオエンジンを殺さないための『隔離』がテーマです。

- **Absolute Decoupling**: UIスレッド（メインスレッド）は、絶対にオーディオスレッドが握りうる `std::mutex` をロックしてはなりません。（過去にこれにより音が頻繁に途切れる放送事故が起きました）。
- **Command Queue Only**: トラックの追加、パラメータのマクロ変更、再生/停止などのコマンドは、すべて `src/core/concurrency/lock_free_command_queue.hpp` を通してのみオーディオ側に伝達すること。
- **60FPS Polling**: メーターや波形描画のアニメーションは、必ず `std::atomic` にキャッシュされた値を定期的にポーリング（読み出し）するプル型アーキテクチャで行うこと。オーディオ側からUI描画をトリガー（プッシュ）してはなりません（フレームレートがオーディオの処理速度に引きずられてUIが固まります）。
