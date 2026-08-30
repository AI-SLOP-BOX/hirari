#pragma once
#include <vector>
#include <algorithm>
#include <array>
#include <atomic>
#include <set>
#include <limits>
#include <map>
#include <mutex>
#include <thread>

#include "pdc_graph.hpp"

namespace Aura::Core::Engine {

/**
 * @class PDCManager
 * @brief Professional DAG-based Plug-in Delay Compensation (PDC).
 * Implements Recursive Path-Depth traversal to solve latency offsets
 * for complex DAG Routing Graphs (Tracks -> Buses -> Master).
 */
class PDCManager {
public:
    static constexpr size_t kMaxTracks = 512;
    static constexpr size_t kMaxBuses = 128;
    static constexpr uint32_t kMasterID = 0xFFFFFFFF;

    static PDCManager& getInstance() { static PDCManager i; return i; }

    uint32_t getCompensationOffset(uint32_t trackId) const {
        uint32_t idx = m_activeBuffer.load(std::memory_order_acquire);
        return (trackId < kMaxTracks) ? m_trackOffsets[idx][trackId].load(std::memory_order_acquire) : 0;
    }

    uint32_t getGlobalMaxLatency() const { return m_maxGlobal.load(std::memory_order_acquire); }
    uint32_t getMaxLatency() const { return getGlobalMaxLatency(); }
    bool hasCycle() const { return m_cycleDetected.load(std::memory_order_acquire); }
    bool lowLatencyMode() const { return m_lowLatencyMode.load(std::memory_order_acquire); }
    uint64_t configurationGeneration() const {
        return m_configurationGeneration.load(std::memory_order_acquire);
    }

    bool bindControlThread() noexcept {
        const uint64_t token = currentThreadToken();
        uint64_t expected = 0;
        if (m_controlThreadToken.compare_exchange_strong(
                expected, token, std::memory_order_acq_rel)) return true;
        return expected == token;
    }

    uint32_t getBusOffset(uint32_t busId) const {
        uint32_t idx = m_activeBuffer.load(std::memory_order_acquire);
        return (busId < kMaxBuses) ? m_busOffsets[idx][busId].load(std::memory_order_acquire) : 0;
    }

    // --- SETUP: ROUTING GRAPH ---
    bool setTrackDest(uint32_t trackId, uint32_t destId) {
        if (trackId < kMaxTracks && controlThreadAllowed()) {
            std::lock_guard<std::mutex> lock(m_configurationMutex);
            m_routing[trackId].store(destId, std::memory_order_release);
            markDirtyLocked();
            return true;
        }
        return false;
    }

    bool setBusDest(uint32_t busId, uint32_t destId) {
        if (busId < kMaxBuses && controlThreadAllowed()) {
            std::lock_guard<std::mutex> lock(m_configurationMutex);
            m_routing[kMaxTracks + busId].store(destId, std::memory_order_release);
            markDirtyLocked();
            return true;
        }
        return false;
    }

    bool setTrackLatency(uint32_t trackId, uint32_t samples) {
        if (trackId < kMaxTracks && controlThreadAllowed()) {
            std::lock_guard<std::mutex> lock(m_configurationMutex);
            m_trackLatencies[trackId].store(samples, std::memory_order_release);
            markDirtyLocked();
            return true;
        }
        return false;
    }

    bool setBusLatency(uint32_t busId, uint32_t samples) {
        if (busId < kMaxBuses && controlThreadAllowed()) {
            std::lock_guard<std::mutex> lock(m_configurationMutex);
            m_busLatencies[busId].store(samples, std::memory_order_release);
            markDirtyLocked();
            return true;
        }
        return false;
    }

    bool setLowLatencyMode(bool active) {
        if (!controlThreadAllowed()) return false;
        std::lock_guard<std::mutex> lock(m_configurationMutex);
        m_lowLatencyMode.store(active, std::memory_order_release);
        markDirtyLocked();
        return true;
    }
    bool markDirty() {
        if (!controlThreadAllowed()) return false;
        std::lock_guard<std::mutex> lock(m_configurationMutex);
        markDirtyLocked();
        return true;
    }

private:
    static uint64_t currentThreadToken() noexcept {
        static std::atomic<uint64_t> next{1};
        thread_local uint64_t token = 0;
        if (token == 0) token = next.fetch_add(1, std::memory_order_relaxed);
        return token;
    }

    bool controlThreadAllowed() const noexcept {
        const uint64_t bound = m_controlThreadToken.load(std::memory_order_acquire);
        // A zero token means ownership has not been established yet. Do not
        // treat that state as unrestricted: doing so lets an early audio
        // callback enter a mutator and wait on the configuration mutex. The
        // engine binds its control owner during construction; standalone
        // users must do the same before editing the graph.
        return bound != 0 && bound == currentThreadToken();
    }

    void markDirtyLocked() noexcept {
        m_configurationGeneration.fetch_add(1, std::memory_order_acq_rel);
        m_dirty.store(true, std::memory_order_release);
    }

public:

    void resetForProject() {
        if (!controlThreadAllowed()) return;
        std::lock_guard<std::mutex> lock(m_configurationMutex);
        const uint32_t active = m_activeBuffer.load(std::memory_order_acquire);
        const uint32_t writeIdx = 1u - active;
        for (size_t t = 0; t < kMaxTracks; ++t) {
            m_routing[t].store(kMasterID, std::memory_order_release);
            m_trackLatencies[t].store(0, std::memory_order_release);
            m_trackOffsets[writeIdx][t].store(0, std::memory_order_relaxed);
        }
        for (size_t b = 0; b < kMaxBuses; ++b) {
            m_routing[kMaxTracks + b].store(kMasterID, std::memory_order_release);
            m_busLatencies[b].store(0, std::memory_order_release);
            m_busOffsets[writeIdx][b].store(0, std::memory_order_relaxed);
        }
        // Never write the buffer currently visible to the audio callback.
        // Publish the clean inactive snapshot only after all entries are set.
        m_activeBuffer.store(writeIdx, std::memory_order_release);
        m_maxGlobal.store(0, std::memory_order_release);
        m_cycleDetected.store(false, std::memory_order_release);
        markDirtyLocked();
    }


    void recalculate() {
        if (!controlThreadAllowed()) return;
        // Configuration writers use the same control-plane lock. This makes
        // the generation check and the active-buffer/dirty publication one
        // transaction instead of a check-then-store race.
        std::lock_guard<std::mutex> lock(m_configurationMutex);
        if (!m_dirty.load(std::memory_order_acquire)) return;

        // Perform topological path traversal to resolve absolute latencies
        uint32_t writeIdx = 1 - m_activeBuffer.load(std::memory_order_relaxed);
        if (m_lowLatencyMode.load(std::memory_order_acquire)) {
            for (size_t t = 0; t < kMaxTracks; ++t)
                m_trackOffsets[writeIdx][t].store(0, std::memory_order_relaxed);
            for (size_t b = 0; b < kMaxBuses; ++b)
                m_busOffsets[writeIdx][b].store(0, std::memory_order_relaxed);
            m_cycleDetected.store(false, std::memory_order_release);
            m_activeBuffer.store(writeIdx, std::memory_order_release);
            m_maxGlobal.store(0, std::memory_order_release);
            m_dirty.store(false, std::memory_order_release);
            return;
        }

        std::vector<uint32_t> pathLatencies(kMaxTracks + kMaxBuses, 0);
        uint32_t maxGlobal = 0;

        // Latencies are uint32_t throughout the public contract.  An int32
        // memo would turn valid values above INT32_MAX into negative entries
        // and could make a later traversal recompute or misread a path.
        std::vector<uint32_t> memo(kMaxTracks + kMaxBuses, 0);
        std::vector<uint8_t> memoValid(kMaxTracks + kMaxBuses, 0);
        std::set<uint32_t> visited;
        bool cycleDetected = false;
        auto getPath = [&](auto& self, uint32_t nodeId) -> uint32_t {
            if (nodeId == kMasterID) return 0;
            if (visited.count(nodeId)) {
                cycleDetected = true;
                return 0;
            }

            if (nodeId < kMaxTracks + kMaxBuses) {
                if (memoValid[nodeId] != 0) return memo[nodeId];
            }

            visited.insert(nodeId);
            uint32_t local = 0;
            uint32_t dest = kMasterID;

            if (nodeId < kMaxTracks) {
                local = m_trackLatencies[nodeId].load(std::memory_order_relaxed);
                dest = m_routing[nodeId].load(std::memory_order_relaxed);
            } else {
                uint32_t busIdx = nodeId - kMaxTracks;
                if (busIdx < kMaxBuses) {
                    local = m_busLatencies[busIdx].load(std::memory_order_relaxed);
                    dest = m_routing[nodeId].load(std::memory_order_relaxed);
                }
            }

            const uint32_t downstream = self(self, dest);
            const uint32_t total = downstream > std::numeric_limits<uint32_t>::max() - local
                ? std::numeric_limits<uint32_t>::max()
                : local + downstream;
            visited.erase(nodeId);

            if (nodeId < kMaxTracks + kMaxBuses) {
                memo[nodeId] = total;
                memoValid[nodeId] = 1;
            }
            return total;
        };

        // Compute path latencies
        for (uint32_t t = 0; t < kMaxTracks; ++t) {
            visited.clear();
            uint32_t lat = getPath(getPath, t);
            pathLatencies[t] = lat;
            maxGlobal = std::max(maxGlobal, lat);
        }
        for (uint32_t b = 0; b < kMaxBuses; ++b) {
            visited.clear();
            uint32_t lat = getPath(getPath, kMaxTracks + b);
            pathLatencies[kMaxTracks + b] = lat;
            maxGlobal = std::max(maxGlobal, lat);
        }

        if (cycleDetected) {
            // Keep the last valid active snapshot. A transient routing cycle
            // must not cause an audible timing jump by publishing all-zero
            // compensation. The dirty flag is cleared only for this rejected
            // calculation; fixing the route calls markDirtyLocked() again.
            m_cycleDetected.store(true, std::memory_order_release);
            m_dirty.store(false, std::memory_order_release);
            return;
        }

        // Apply compensation offsets (difference to max global latency path)
        for (uint32_t t = 0; t < kMaxTracks; ++t) {
            m_trackOffsets[writeIdx][t].store(maxGlobal - pathLatencies[t], std::memory_order_relaxed);
        }
        for (uint32_t b = 0; b < kMaxBuses; ++b) {
            m_busOffsets[writeIdx][b].store(maxGlobal - pathLatencies[kMaxTracks + b], std::memory_order_relaxed);
        }

        m_cycleDetected.store(false, std::memory_order_release);
        m_activeBuffer.store(writeIdx, std::memory_order_release);
        m_maxGlobal.store(maxGlobal, std::memory_order_release);
        m_dirty.store(false, std::memory_order_release);
    }

    // Cross-check the legacy publication buffers against the shared graph
    // solver. This is intentionally control-rate only and never runs on the
    // audio callback; it prevents the two remaining implementations from
    // silently drifting apart during graph changes.
    bool auditAgainstSharedSolver() const {
        if (!controlThreadAllowed()) return false;
        std::lock_guard<std::mutex> lock(m_configurationMutex);
        std::map<uint32_t, PDCGraphSolver::Node> graph;
        for (uint32_t t = 0; t < kMaxTracks; ++t) {
            const uint32_t dest = m_routing[t].load(std::memory_order_acquire);
            graph.emplace(t, PDCGraphSolver::Node{
                t, m_trackLatencies[t].load(std::memory_order_acquire), 0, 0, true,
                dest == kMasterID ? std::vector<uint32_t>{} : std::vector<uint32_t>{dest}});
        }
        for (uint32_t b = 0; b < kMaxBuses; ++b) {
            const uint32_t id = kMaxTracks + b;
            const uint32_t dest = m_routing[id].load(std::memory_order_acquire);
            graph.emplace(id, PDCGraphSolver::Node{
                id, m_busLatencies[b].load(std::memory_order_acquire), 0, 0, true,
                dest == kMasterID ? std::vector<uint32_t>{} : std::vector<uint32_t>{dest}});
        }
        PDCGraphSolver solver;
        solver.solve(graph);
        if (solver.hasCycle()) return hasCycle();
        if (solver.globalProjectLatency() != getGlobalMaxLatency()) return false;
        const uint32_t active = m_activeBuffer.load(std::memory_order_acquire);
        for (uint32_t t = 0; t < kMaxTracks; ++t) {
            if (m_trackOffsets[active][t].load(std::memory_order_acquire) != graph.at(t).compensation) return false;
        }
        for (uint32_t b = 0; b < kMaxBuses; ++b) {
            if (m_busOffsets[active][b].load(std::memory_order_acquire) != graph.at(kMaxTracks + b).compensation) return false;
        }
        return true;
    }

public:
    PDCManager() {
        m_activeBuffer.store(0);
        m_maxGlobal.store(0);
        m_dirty.store(true);
        m_lowLatencyMode.store(false);
        for (size_t t = 0; t < kMaxTracks; ++t) {
            m_trackLatencies[t].store(0);
            m_trackOffsets[0][t].store(0, std::memory_order_relaxed);
            m_trackOffsets[1][t].store(0, std::memory_order_relaxed);
            m_routing[t].store(kMasterID);
        }
        for (size_t b = 0; b < kMaxBuses; ++b) {
            m_busLatencies[b].store(0);
            m_busOffsets[0][b].store(0, std::memory_order_relaxed);
            m_busOffsets[1][b].store(0, std::memory_order_relaxed);
            m_routing[kMaxTracks + b].store(kMasterID);
        }
    }

private:

    mutable std::mutex m_configurationMutex;
    std::atomic<uint64_t> m_controlThreadToken{0};
    std::atomic<bool> m_dirty{true};
    std::atomic<uint64_t> m_configurationGeneration{0};
    std::atomic<bool> m_lowLatencyMode{false};
    std::atomic<uint32_t> m_activeBuffer{0};
    std::atomic<uint32_t> m_maxGlobal{0};
    std::atomic<bool> m_cycleDetected{false};

    std::atomic<uint32_t> m_trackOffsets[2][kMaxTracks];
    std::atomic<uint32_t> m_busOffsets[2][kMaxBuses];

    std::atomic<uint32_t> m_trackLatencies[kMaxTracks];
    std::atomic<uint32_t> m_busLatencies[kMaxBuses];
    std::atomic<uint32_t> m_routing[kMaxTracks + kMaxBuses];
};

} // namespace Aura::Core::Engine
