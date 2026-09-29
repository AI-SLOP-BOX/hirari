# SKILL: Hirari Graphics (GPU Hardware Acceleration)

## 🎨 Metal/Vulkan Drawing Directives
DAWが重くなる原因の50%は「波形の描画」です。オーディオ処理の邪魔をしないためのグラフィックルールの徹底。

- **No CPU Audio Drawing**: 数時間・数百万サンプルに及ぶ巨大なオーディオクリップ（波形）の描画において、CPUの `drawLine` 等でピクセル単位の線を描いてはならない。波形のMin/MaxをMipmap化（ズームレベル別の事前計算）し、一括でGPUの頂点バッファに転送すること。
- **Goniometer / Analyzer**: アナライザやリサジュー図形（フェーズメーター）、スペクトラム等の高フレームレートが要求されるメーター群は、必ず 60FPS でシェーダーを通して描画すること。CPU側での頂点計算は最小限に留める。
