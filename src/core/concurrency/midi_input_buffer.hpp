#pragma once

#include <atomic>
#include <array>
#include <vector>
#include <cstdint>

namespace Aura::Core::Concurrency {

/**
 * @brief MidiInputBuffer: 【完全ロックフリー】リアルタイム・ハードウェアMIDIキャプチャ
 * 以前のバージョンは悪名高い「偽のリアルタイム安全（Fake Real-Time Safe）」でした。
 * OSのMIDI割り込みスレッドと、DAWの1分1秒を争うAudioスレッド間で `std::mutex` を使ってロックしていたため、
 * タイミング悪く待機が重なると、音切れ（デジタルのプツッというノイズ）が100%発生する構造的欠陥がありました。
 * さらには `std::vector::push_back` でメモリを動的確保するというリアルタイムプログラミングの「最悪の禁じ手」も犯していました。
 *
 * 【最新修正版】はミューテックス（排他制御）をメモリ空間から完全に排除し、固定長のリングバッファ（Circular Buffer）と
 * `std::atomic` メモリオペレーションを用いたSPSC（Single Producer, Single Consumer）キューとして完全に再設計されています。
 */
class MidiInputBuffer {
public:
    static MidiInputBuffer& getInstance() {
        static MidiInputBuffer instance;
        return instance;
    }

    struct RawMidiEvent {
        uint8_t status;
        uint8_t data1;
        uint8_t data2;
        uint64_t timestamp;
    };

    /**
     * @brief 外部MIDIハードウェア（USBキーボード等）から押し込まれる（プロデューサー・スレッド）
     * メモリ割り当て（new）も、スレッドロック（mutex）も一切行わず、ナノ秒レベルで書き込みを完了させます。
     */
    void pushEvent(uint8_t s, uint8_t d1, uint8_t d2, uint64_t ts) {
        if (!acceptEvent(s, d1, d2, ts)) {
            m_droppedEvents.fetch_add(1, std::memory_order_relaxed);
            return;
        }
        size_t currentWrite = m_writeIndex.load(std::memory_order_relaxed);
        size_t nextWrite = (currentWrite + 1) & (MAX_MIDI_EVENTS - 1);

        if (nextWrite != m_readIndex.load(std::memory_order_acquire)) {
            m_ringBuffer[currentWrite] = {s, d1, d2, ts};
            m_writeIndex.store(nextWrite, std::memory_order_release);
        }
    }

    struct BlacklistRule { uint8_t statusMask; uint8_t statusValue; uint8_t data1; uint8_t data2; };

    bool addBlacklistRule(uint8_t statusMask, uint8_t statusValue, uint8_t data1 = 0xFF, uint8_t data2 = 0xFF) {
        const size_t index = m_ruleCount.load(std::memory_order_relaxed);
        if (index >= kMaxRules) return false;
        m_rules[index] = {statusMask, statusValue, data1, data2};
        m_ruleCount.store(index + 1, std::memory_order_release);
        return true;
    }

    void clearBlacklist() { m_ruleCount.store(0, std::memory_order_release); }
    uint64_t droppedEventCount() const { return m_droppedEvents.load(std::memory_order_relaxed); }

    size_t pullEvents(std::array<RawMidiEvent, 512>& outputBuffer) {
        size_t currentRead = m_readIndex.load(std::memory_order_relaxed);
        size_t currentWrite = m_writeIndex.load(std::memory_order_acquire);
        
        size_t count = 0;
        while (currentRead != currentWrite && count < outputBuffer.size()) {
            outputBuffer[count] = m_ringBuffer[currentRead];
            currentRead = (currentRead + 1) & (MAX_MIDI_EVENTS - 1);
            count++;
        }
        
        if (count > 0) {
            m_readIndex.store(currentRead, std::memory_order_release);
        }
        
        return count;
    }

private:
    MidiInputBuffer() : m_writeIndex(0), m_readIndex(0) {}

    bool acceptEvent(uint8_t status, uint8_t data1, uint8_t data2, uint64_t timestamp) {
        const size_t ruleCount = m_ruleCount.load(std::memory_order_acquire);
        for (size_t i = 0; i < ruleCount; ++i) {
            const auto& rule = m_rules[i];
            if ((status & rule.statusMask) == rule.statusValue &&
                (rule.data1 == 0xFF || rule.data1 == data1) &&
                (rule.data2 == 0xFF || rule.data2 == data2)) return false;
        }

        // Limit bursts per status/channel. Timestamps are expected to use a
        // monotonic unit; a one-second window keeps runaway devices bounded.
        const uint8_t bucket = status & 0x0F;
        const uint64_t windowStart = m_rateWindowStart[bucket].load(std::memory_order_relaxed);
        if (timestamp >= windowStart + kRateWindow || timestamp < windowStart) {
            m_rateWindowStart[bucket].store(timestamp, std::memory_order_relaxed);
            m_rateCount[bucket].store(0, std::memory_order_relaxed);
        }
        const uint32_t count = m_rateCount[bucket].fetch_add(1, std::memory_order_relaxed) + 1;
        return count <= kMaxEventsPerWindow;
    }

    static constexpr size_t MAX_MIDI_EVENTS = 2048;
    std::array<RawMidiEvent, MAX_MIDI_EVENTS> m_ringBuffer;
    
    alignas(64) std::atomic<size_t> m_writeIndex;
    alignas(64) std::atomic<size_t> m_readIndex;
    static constexpr size_t kMaxRules = 64;
    static constexpr uint64_t kRateWindow = 1000000;
    static constexpr uint32_t kMaxEventsPerWindow = 20000;
    std::array<BlacklistRule, kMaxRules> m_rules{};
    std::atomic<size_t> m_ruleCount{0};
    std::array<std::atomic<uint64_t>, 16> m_rateWindowStart{};
    std::array<std::atomic<uint32_t>, 16> m_rateCount{};
    std::atomic<uint64_t> m_droppedEvents{0};
};

} // namespace Aura::Core::Concurrency
