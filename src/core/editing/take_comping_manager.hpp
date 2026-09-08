#pragma once
#include <vector>
#include <array>
#include <memory>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include "../audio_region.hpp"

namespace Aura::Core::Editing {

/**
 * @struct CompRegion
 * @brief コンピング専用のオーディオ区間データ
 */
struct CompRegion {
    std::shared_ptr<AudioRegion> sourceTake; // どのテイク由来か
    uint64_t startSample;       // タイムライン上の開始位置
    uint64_t endSample;         // タイムライン上の終了位置
    uint32_t crossfadeSamples;  // 前後との自動クロスフェード長
};

/**
 * @class TakeCompingManager
 * @brief 【超絶肉付け・王道DAW必須機能】Logic Proの「Quick Swipe Comping（テイク・フォルダ）」
 * ボーカリストに同じサビを10回歌わせ（ループ録音）、
 * 「1回目はAメロの出だしが良い」「3回目はサビのピッチが完璧」といった風に、
 * 各テイクの一番良い部分だけをマウスで「スワイプ（なぞる）」して繋ぎ合わせ、
 * 繋ぎ目の「ブツッ」というノイズを自動クロスフェード（フェードエンベロープ）で消し去り、
 * 1つの完璧な架空のボーカルトラック（コンポジット）を作り上げる、絶対に不可欠な編集コアです。
 */
class TakeCompingManager {
public:
    TakeCompingManager(uint32_t defaultXfade = 512) : m_defaultCrossfadeSamples(defaultXfade) {}

    void setCrossfadeSamples(uint32_t samples) {
        m_defaultCrossfadeSamples = std::clamp(samples, 0u, 10000u);
    }
    uint32_t getCrossfadeSamples() const { return m_defaultCrossfadeSamples; }

    void addTake(std::shared_ptr<AudioRegion> newTake) {
        m_takes.push_back(newTake);
    }

    /**
     * @brief ユーザーが「このテイクのこの部分を使う！」とスワイプ（選択）した際の処理
     */
    void swipeTakeRegion(size_t takeIndex, uint64_t start, uint64_t end) {
        if (takeIndex >= m_takes.size() || start >= end || !m_takes[takeIndex]) return;

        // 【アルゴリズム的解決】
        // 既存のコンピング（選択）領域と時間が被っている場合、
        // 重なっている部分を数学的に分割（Split）し、新しいテイクで上書きします。
        std::vector<CompRegion> newComps;
        
        for (auto& existing : m_activeRegions) {
            // 被っていない領域はそのまま残す
            if (existing.endSample <= start || existing.startSample >= end) {
                newComps.push_back(existing);
                continue;
            }

            // [既存]が[新規]より前から始まっていれば、前半を残す
            if (existing.startSample < start) {
                newComps.push_back({existing.sourceTake, existing.startSample, start, m_defaultCrossfadeSamples});
            }

            // [既存]が[新規]より後まで続いていれば、後半を残す
            if (existing.endSample > end) {
                newComps.push_back({existing.sourceTake, end, existing.endSample, m_defaultCrossfadeSamples});
            }
        }

        // ユーザーが新しくなぞった（スワイプした）一番美味しい部分を挿入
        newComps.push_back({m_takes[takeIndex], start, end, m_defaultCrossfadeSamples});

        // タイムライン順にソート（再生エンジン用）
        std::sort(newComps.begin(), newComps.end(), [](const CompRegion& a, const CompRegion& b) {
            return a.startSample < b.startSample;
        });

        m_activeRegions = newComps;
    }

    /**
     * @brief オーディオスレッドから呼ばれて、結合された「最強のコンポジット波形」を返す
     */
    void renderComposite(float* outBuffer, uint64_t renderStart, uint32_t numSamples) {
        if (outBuffer == nullptr || numSamples == 0) return;

        std::fill(outBuffer, outBuffer + numSamples, 0.0f);

        constexpr uint32_t kMaxSamples = 4096;
        static thread_local std::array<float, kMaxSamples> tempL;
        static thread_local std::array<float, kMaxSamples> tempR;
        for (uint32_t base = 0; base < numSamples;) {
            const uint32_t processCount = std::min(kMaxSamples, numSamples - base);
            const uint64_t chunkStart = renderStart > UINT64_MAX - base
                ? UINT64_MAX : renderStart + base;
            const uint64_t chunkEnd = chunkStart > UINT64_MAX - processCount
                ? UINT64_MAX : chunkStart + processCount;
            for (const auto& reg : m_activeRegions) {
                if (!reg.sourceTake || reg.endSample <= chunkStart || reg.startSample >= chunkEnd) continue;
                const uint64_t start = std::max(chunkStart, reg.startSample);
                const uint64_t end = std::min(chunkEnd, reg.endSample);
                const uint32_t copyLen = static_cast<uint32_t>(end - start);
                if (copyLen == 0) continue;
                const uint32_t offset = base + static_cast<uint32_t>(start - chunkStart);
                std::fill(tempL.begin(), tempL.begin() + copyLen, 0.0f);
                std::fill(tempR.begin(), tempR.begin() + copyLen, 0.0f);
                reg.sourceTake->render(tempL.data(), tempR.data(), start, copyLen);
                const float xfadeSamples = static_cast<float>(reg.crossfadeSamples);
                for (uint32_t s = 0; s < copyLen; ++s) {
                    const uint64_t position = start + s;
                    float fade = 1.0f;
                    if (reg.crossfadeSamples > 0 && position - reg.startSample < reg.crossfadeSamples) {
                        fade = std::sin(1.57079632679f * static_cast<float>(position - reg.startSample) / xfadeSamples);
                    } else if (reg.crossfadeSamples > 0 && reg.endSample - position < reg.crossfadeSamples) {
                        fade = std::sin(1.57079632679f * static_cast<float>(reg.endSample - position) / xfadeSamples);
                    }
                    outBuffer[offset + s] += tempL[s] * fade;
                }
            }
            if (processCount == numSamples - base) break;
            base += processCount;
        }
    }

    // Stereo counterpart used by the track renderer.  Keep the legacy mono
    // entrypoint above for downstream callers, but do not silently discard
    // the right channel when a comp is rendered into a stereo track.
    void renderCompositeStereo(float* outLeft, float* outRight,
                               uint64_t renderStart, uint32_t numSamples) {
        if (!outLeft || !outRight || numSamples == 0) return;
        std::fill(outLeft, outLeft + numSamples, 0.0f);
        std::fill(outRight, outRight + numSamples, 0.0f);
        static thread_local std::array<float, kMaxRenderSamples> tempLeft{};
        static thread_local std::array<float, kMaxRenderSamples> tempRight{};
        for (uint32_t base = 0; base < numSamples;) {
            const uint32_t processCount = std::min(kMaxRenderSamples, numSamples - base);
            const uint64_t chunkStart = renderStart > UINT64_MAX - base
                ? UINT64_MAX : renderStart + base;
            const uint64_t windowEnd = chunkStart > UINT64_MAX - processCount
                ? UINT64_MAX : chunkStart + processCount;
            for (const auto& reg : m_activeRegions) {
                if (!reg.sourceTake || reg.endSample <= chunkStart || reg.startSample >= windowEnd) continue;
                const uint64_t start = std::max(chunkStart, reg.startSample);
                const uint64_t end = std::min(windowEnd, reg.endSample);
                const uint32_t count = static_cast<uint32_t>(end - start);
                if (count == 0) continue;
                const uint32_t offset = base + static_cast<uint32_t>(start - chunkStart);
                std::fill(tempLeft.begin(), tempLeft.begin() + count, 0.0f);
                std::fill(tempRight.begin(), tempRight.begin() + count, 0.0f);
                reg.sourceTake->render(tempLeft.data(), tempRight.data(), start, count);
                for (uint32_t s = 0; s < count; ++s) {
                    const uint64_t position = start + s;
                    float fade = 1.0f;
                    if (reg.crossfadeSamples > 0 && position - reg.startSample < reg.crossfadeSamples) {
                        fade = std::sin(1.57079632679f * static_cast<float>(position - reg.startSample) /
                                        static_cast<float>(reg.crossfadeSamples));
                    } else if (reg.crossfadeSamples > 0 && reg.endSample - position < reg.crossfadeSamples) {
                        fade = std::sin(1.57079632679f * static_cast<float>(reg.endSample - position) /
                                        static_cast<float>(reg.crossfadeSamples));
                    }
                    outLeft[offset + s] += tempLeft[s] * fade;
                    outRight[offset + s] += tempRight[s] * fade;
                }
            }
            if (processCount == numSamples - base) break;
            base += processCount;
        }
    }

    const std::vector<CompRegion>& getActiveRegions() const { return m_activeRegions; }
    void clearComp() { m_activeRegions.clear(); }

private:
    static constexpr uint32_t kMaxRenderSamples = 4096;
    std::vector<std::shared_ptr<AudioRegion>> m_takes; // 失敗も含めた全録音テイク
    std::vector<CompRegion> m_activeRegions; // ユーザーがなぞって選択した「最強のキメラ」領域
    uint32_t m_defaultCrossfadeSamples = 512;
};

} // namespace Aura::Core::Editing
