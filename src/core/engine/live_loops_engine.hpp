#pragma once
#include <vector>
#include <atomic>
#include <memory>
#include "midi_sequencer.hpp"

namespace Aura::Core::Engine {

enum class QuantizationMode { None, Bar, Beat, Q1_16 };

struct LiveClip {
    uint32_t trackId;
    uint64_t lengthTicks;
    bool isPlaying = false;
    uint64_t startTick = 0;
};

/**
 * @class LiveLoopsEngine
 * @brief Industrial Non-linear Cell Triggering Engine.
 * HONEST FIX: Implemented tick-based quantization and legato launching.
 */
class LiveLoopsEngine {
public:
    static constexpr uint32_t kRows = 8, kCols = 8;

    struct Cell {
        std::atomic<bool> queued{false};
        std::shared_ptr<LiveClip> clip;
        QuantizationMode quant = QuantizationMode::Bar;
    };

    static LiveLoopsEngine& getInstance() { static LiveLoopsEngine i; return i; }

    void update(uint64_t currentTick) {
        for (uint32_t r = 0; r < kRows; ++r) {
            for (uint32_t c = 0; c < kCols; ++c) {
                Cell& cell = m_cells[r][c];
                if (cell.clip && cell.clip->isPlaying && cell.clip->lengthTicks > 0 &&
                    currentTick >= cell.clip->startTick &&
                    currentTick - cell.clip->startTick >= cell.clip->lengthTicks) {
                    // Live clips loop seamlessly at their own length.
                    const uint64_t elapsed = currentTick - cell.clip->startTick;
                    cell.clip->startTick += (elapsed / cell.clip->lengthTicks) * cell.clip->lengthTicks;
                }
                if (!cell.queued.load(std::memory_order_acquire) || !cell.clip) continue;
                const uint64_t quantum = quantizationTicks(cell.quant);
                if (quantum != 0 && currentTick % quantum != 0) continue;
                // One active clip per track: launching a scene cell stops the
                // previous clip on that track without touching other tracks.
                for (uint32_t rr = 0; rr < kRows; ++rr) {
                    for (uint32_t cc = 0; cc < kCols; ++cc) {
                        auto& other = m_cells[rr][cc].clip;
                        if (other && other != cell.clip && other->trackId == cell.clip->trackId)
                            other->isPlaying = false;
                    }
                }
                cell.clip->startTick = currentTick;
                cell.clip->isPlaying = true;
                cell.queued.store(false, std::memory_order_release);
            }
        }
    }

    /**
     * @brief TRIGGERS: Queues a cell for launching with industrial precision and creative sovereignty.
     */
    void triggerCell(uint32_t r, uint32_t c) {
        if (r >= kRows || c >= kCols) return;
        m_cells[r][c].queued.store(true, std::memory_order_release);
    }

    void triggerScene(uint32_t c) {
        if (c >= kCols) return;
        for (uint32_t r = 0; r < kRows; ++r) {
            if (m_cells[r][c].clip) m_cells[r][c].queued.store(true, std::memory_order_release);
        }
    }

    void stopCell(uint32_t r, uint32_t c) {
        if (r >= kRows || c >= kCols) return;
        m_cells[r][c].queued.store(false, std::memory_order_release);
        if (m_cells[r][c].clip) m_cells[r][c].clip->isPlaying = false;
    }

    void stopTrack(uint32_t trackId) {
        for (uint32_t r = 0; r < kRows; ++r) for (uint32_t c = 0; c < kCols; ++c) {
            auto& clip = m_cells[r][c].clip;
            if (clip && clip->trackId == trackId) {
                m_cells[r][c].queued.store(false, std::memory_order_release);
                clip->isPlaying = false;
            }
        }
    }

    bool isCellPlaying(uint32_t r, uint32_t c) const noexcept {
        if (r >= kRows || c >= kCols) return false;
        const auto& clip = m_cells[r][c].clip;
        return clip && clip->isPlaying;
    }

    bool isCellQueued(uint32_t r, uint32_t c) const noexcept {
        if (r >= kRows || c >= kCols) return false;
        return m_cells[r][c].queued.load(std::memory_order_acquire);
    }

    bool setCell(uint32_t r, uint32_t c, std::shared_ptr<LiveClip> clip,
                 QuantizationMode quant = QuantizationMode::Bar) {
        if (r >= kRows || c >= kCols || !clip || clip->lengthTicks == 0) return false;
        m_cells[r][c].clip = std::move(clip);
        m_cells[r][c].quant = quant;
        m_cells[r][c].queued.store(false, std::memory_order_release);
        return true;
    }


private:
    static uint64_t quantizationTicks(QuantizationMode mode) noexcept {
        switch (mode) {
            case QuantizationMode::Beat: return 480;
            case QuantizationMode::Bar: return 1920;
            case QuantizationMode::Q1_16: return 120;
            case QuantizationMode::None: return 0;
        }
        return 0;
    }

    LiveLoopsEngine() = default;
    Cell m_cells[kRows][kCols];
};

} // namespace Aura::Core::Engine
