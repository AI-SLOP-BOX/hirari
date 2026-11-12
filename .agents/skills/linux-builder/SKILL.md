---
name: linux-remote-builder
description: Linux PC (192.168.0.21, aislopbox) を使ったリモートビルド・自動テスト・GUI/ホストアプリ実行と CI ログ確認。Mac のローカルストレージ (target/) を消費せず高速にビルド・テストを回す際に使用。
---

# Linux リモートビルダー＆ホスト実行運用ガイド

Mac 本体のストレージ（target/ 肥大化）を節約し、常時稼働の Linux ビルドサーバー（`192.168.0.21` / 462GB NVMe / Fedora 45）にビルド・テスト・GUI ホスト実行をオフロードする手順です。

## 接続情報
- **ホスト**: `192.168.0.21` (orenopc)
- **ユーザー**: `aislopbox`
- **SSH 認証**: パスワードなし鍵認証 (`~/.ssh/id_ed25519`)
- **sudo 権限**: NOPASSWD 設定済み（`ssh aislopbox@192.168.0.21 "sudo ..."` が非対話で実行可能）
- **常時稼働**: OS レベルで sleep / suspend / hibernate 完全無効化済み（ディスプレイのみ消灯）
- **ホスト環境**: Xvfb (仮想 X11 ディスプレイ), ALSA / PulseAudio / PipeWire, Vulkan (Mesa) 導入済み

## リモート push と CI 自動テスト
各プロジェクト（AmeUI, Hirari, Kagari）のリポジトリ内で以下のコマンドを実行すると、Linux 側の CI リポジトリへコードが送信され、自動で `cargo check` および `xvfb-run -a cargo test` が走ります。

```bash
# 短縮エイリアスで push (HEAD をリモートの main に送信)
git pushlinux

# または通常の git push
git push linux main
```

## CI ログの確認
ビルド・テスト結果は Linux 側のプロジェクト別ログファイルに記録されます。Mac のターミナルから直接確認できます。

```bash
# 最新のログを確認 (例: ameui, kagari, hirari)
ssh aislopbox@192.168.0.21 "tail -n 50 ~/ci-logs/ameui.log"

# 末尾をリアルタイム監視 (Ctrl+C で終了)
ssh aislopbox@192.168.0.21 "tail -f ~/ci-logs/ameui.log"
```

## Linux 側でのホストアプリ実行 (GUI / DAW / テストランナー)

### 1. 仮想ディスプレイ (Headless) でホスト実行
X11 ディスプレイをエミュレートする `xvfb-run` がセットアップされているため、UI ウィンドウ生成や WGPU レンダリングを伴うホストアプリも SSH 経由でバックグラウンド実行・テストできます。

```bash
# AmeUI のホスト／テスト実行
ssh aislopbox@192.168.0.21 "cd ~/work/ameui && xvfb-run -a cargo run --example <example_name>"

# Kagari / Hirari ホストの実行
ssh aislopbox@192.168.0.21 "cd ~/work/kagari && xvfb-run -a cargo run --bin kagari-studio"
```

### 2. Linux 本体の実画面（ディスプレイ）に GUI を表示して実行
Linux PC に接続されているモニターに直接ウィンドウを描画したい場合は、ログインセッションの `DISPLAY`（通常 `:0`）または Wayland を指定して実行します。

```bash
# Linux 実画面に表示
ssh aislopbox@192.168.0.21 "cd ~/work/ameui && DISPLAY=:0 cargo run --example <example_name>"
```

### 3. Mac 側へ GUI 画面を X11 転送して手元に表示
Mac 側に XQuartz が入っている場合、`-X` または `-Y` オプションをつけて SSH 接続することで、Linux 側で動く GUI ウィンドウを手元の Mac 画面上にそのまま転送して操作できます。

```bash
ssh -Y aislopbox@192.168.0.21 "cd ~/work/ameui && cargo run --example <example_name>"
```

## 新しいプロジェクトの CI リポジトリ追加
新しいプロジェクト（例: `my-project`）を Linux サーバーに追加したいときは、Linux 側で用意した自動初期化スクリプトを実行するだけで完了します。

```bash
# 1. Linux 側で CI リポジトリとワークツリーを作成
ssh aislopbox@192.168.0.21 "~/bin/setup-ci-repo.sh <project-name>"

# 2. Mac 側のプロジェクトに linux リモートを追加
cd /Users/dev/Desktop/AI-SLOP-Family/<project-name>
git remote add linux aislopbox@192.168.0.21:~/repos/<project-name>.git
git config alias.pushlinux "push linux HEAD:main"

# 3. push してビルド開始
git pushlinux
```

## Mac 側ローカルのストレージ開放
Linux 側でビルドが完了した後は、Mac 側の巨大な `target/` ディレクトリを安全にクリーンアップできます。

```bash
cargo clean
```
