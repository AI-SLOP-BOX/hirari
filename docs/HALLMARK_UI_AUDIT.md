# Aura UI / Hallmark study

参照: [Nutlope/hallmark](https://github.com/Nutlope/hallmark)

この文書は、Hallmarkの「AI生成UIらしさを避ける」設計原則をAura DAWの既存UIへ適用するための実装基準です。HallmarkはWeb向けの設計スキルなので、Slint固有の記法ではなく、原則だけを移植します。

## 適用する原則

1. **構造を先に設計する**

   ページごとに異なる目的と操作密度を持たせる。同じ見出し・同じ3カード・同じステータス行を全ページへ複製しない。

2. **数値を捏造しない**

   エンジンから取得できないCPU、レイテンシー、プラグイン数、メモリ量は表示しない。未接続状態は `—` または `Not connected` と表示する。

3. **デザイントークンを経由する**

   色、フォント、境界線、余白は `style.slint` のトークンを使用する。新しい色をコンポーネントへ直接書かない。

4. **操作状態を明示する**

   ボタン、セル、ノブ、スライダーは default / hover / focus / active / disabled を区別する。色だけに依存せず、境界線やラベルでも状態を伝える。

5. **ページ追加は情報設計を伴わせる**

   `Arrange`, `Mix`, `Edit`, `Browse`, `Project`, `Settings` を主要ナビゲーションとし、Flex、Comping、Step Sequencerなどは関連ページ内のサブビューとして扱う。横一列に全機能を並べない。

## 現在の改善対象

- `Z_WorkflowPage` は汎用ページとして残すが、主要ページでは専用レイアウトへ置き換える。
- `Browser` は `Z_LibraryPage` として一覧・検索・Previewを持つ専用構造に変更済み。
- `Quick Help` はポップオーバーとしてControl Barから開く。
- LCDはBeats / Time / SMPTEを切り替え可能にする。
- トラックヘッダーはOn/Off、Input Monitoring、Mute、Solo、Recordを分離する。
- `未対応` の長いラベルをボタンへ詰め込まず、短いラベルと状態表示を分離する。

## UIレビュー時のチェックリスト

- [ ] 画面内に同じ目的のメーターが複数ない
- [ ] 固定幅の文字列が隣のコントロールへ侵入しない
- [ ] ページ固有の主要操作が最初の画面で分かる
- [ ] 未接続値を実在する値として表示していない
- [ ] 追加色が `style.slint` のトークンに登録されている
- [ ] 8px未満の主要テキストがない
- [ ] ページ追加が単なるカード複製になっていない
- [ ] 実データがない場合の空状態が明示されている

## 実装順

1. 主要ナビゲーションを6項目へ整理
2. Arrange / Mix / Editを専用レイアウト化
3. Library / Project / Settingsを専用レイアウト化
4. Flex / Comping / Step SequencerをEdit内のサブビューへ整理
5. 実データ未接続の表示を監査
6. 320px相当の狭い領域で文字と操作部品の衝突を確認

