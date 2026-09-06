#pragma once
#include <cstdint>
#include <vector>
#include <memory>
#include <mutex>
#include <atomic>
#include <algorithm>
#include <cmath>
#include <array>
#include <limits>

namespace Aura::Core::Engine {

/**
 * @class RoutingEngine
 * @brief THE NERVE SYSTEM.
 * Manages audio bus routing, sidechains, and parallel processing chains.
 */
class RoutingEngine {
public:
    static RoutingEngine& getInstance() { static RoutingEngine engine; return engine; }
    struct Connection {
        uint32_t sourceId;
        uint32_t destId;
        float gain = 1.0f;
    };

    struct FeedbackConnection {
        uint32_t sourceId = 0;
        uint32_t destId = 0;
        float gain = 1.0f;
    };

    /**
     * @brief CONNECT: Adds a new signal connection with industrial precision and signal sovereignty.
     * INDUSTRIAL: Delegating connection management and sidechain resolution to the Rust 'RoutingOrchestrator'.
     */
    void addConnection(uint32_t s, uint32_t d, float g = 1.0f) {
        if (s == d || s >= kMaxNodes || d >= kMaxNodes || !std::isfinite(g)) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = std::find_if(m_connections.begin(), m_connections.end(),
            [&](const Connection& c) { return c.sourceId == s && c.destId == d; });
        if (it == m_connections.end() && wouldCreateCycleLocked(s, d)) return;
        const float gain = std::clamp(g, 0.0f, 2.0f);
        m_gains[s][d].store(gain, std::memory_order_release);
        if (it != m_connections.end()) it->gain = gain;
        else m_connections.push_back({s, d, gain});
        m_dirty.store(true, std::memory_order_release);
    }

    void removeConnection(uint32_t s, uint32_t d) {
        if (s >= kMaxNodes || d >= kMaxNodes) return;
        m_gains[s][d].store(0.0f, std::memory_order_release);
        std::lock_guard<std::mutex> lock(m_mutex);
        m_connections.erase(std::remove_if(m_connections.begin(), m_connections.end(),
            [&](const Connection& c) { return c.sourceId == s && c.destId == d; }),
            m_connections.end());
        m_dirty.store(true, std::memory_order_release);
    }

    void resetForProject() {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_connections.clear();
        for (auto& row : m_gains) {
            for (auto& gain : row) gain.store(0.0f, std::memory_order_release);
        }
        for (auto& edge : m_feedback) edge.active.store(false, std::memory_order_release);
        m_topologyCount.store(0, std::memory_order_release);
        m_dirty.store(true, std::memory_order_release);
    }


    bool hasConnection(uint32_t s, uint32_t d) const {
        if (s >= kMaxNodes || d >= kMaxNodes) return false;
        return m_gains[s][d].load(std::memory_order_acquire) > 0.0f;
        /*
        std::lock_guard<std::mutex> lock(m_mutex);
        return std::any_of(m_connections.begin(), m_connections.end(),
            [&](const Connection& c) { return c.sourceId == s && c.destId == d; });
        */
    }

    // Control-thread snapshot used by PDC and graph compilation. The audio
    // callback consumes only the already-published topology/gain arrays.
    void copyConnections(std::vector<Connection>& out) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        out = m_connections;
    }

    float connectionGain(uint32_t s, uint32_t d) const noexcept {
        if (s >= kMaxNodes || d >= kMaxNodes) return 0.0f;
        return m_gains[s][d].load(std::memory_order_acquire);
    }

    // Direct-routing fanout for the audio callback.  The destination list is
    // supplied by the compiled graph/UI snapshot; this method performs no
    // allocation or locking and applies every active send sample-accurately.
    void fanoutStereo(uint32_t sourceId, const float* left, const float* right,
                      float* const* destinationLeft, float* const* destinationRight,
                      const uint32_t* destinationIds, uint32_t destinationCount,
                      uint32_t frames) const noexcept {
        if (sourceId >= kMaxNodes || !left || !right || !destinationLeft ||
            !destinationRight || !destinationIds || destinationCount == 0 || frames == 0)
            return;
        const uint32_t count = std::min(destinationCount, kMaxNodes);
        for (uint32_t d = 0; d < count; ++d) {
            const uint32_t destination = destinationIds[d];
            if (destination >= kMaxNodes || !destinationLeft[d] || !destinationRight[d]) continue;
            const float gain = m_gains[sourceId][destination].load(std::memory_order_acquire);
            if (!(gain > 0.0f) || !std::isfinite(gain)) continue;
            for (uint32_t i = 0; i < frames; ++i) {
                const float l = std::isfinite(left[i]) ? left[i] : 0.0f;
                const float r = std::isfinite(right[i]) ? right[i] : 0.0f;
                destinationLeft[d][i] += l * gain;
                destinationRight[d][i] += r * gain;
            }
        }
    }

    // Channel-interleaved variant used by immersive/ADM buses.  Each entry in
    // the channel arrays points to one planar channel; the same direct-route
    // gain is applied to every channel without allocating on the RT thread.
    void fanoutPlanar(uint32_t sourceId, const float* const* sourceChannels,
                      float* const* const* destinationChannels,
                      const uint32_t* destinationIds, uint32_t destinationCount,
                      uint32_t channelCount, uint32_t frames) const noexcept {
        if (sourceId >= kMaxNodes || !sourceChannels || !destinationChannels ||
            !destinationIds || destinationCount == 0 || channelCount == 0 || frames == 0)
            return;
        const uint32_t count = std::min(destinationCount, kMaxNodes);
        const uint32_t channels = std::min(channelCount, 32u);
        for (uint32_t d = 0; d < count; ++d) {
            const uint32_t destination = destinationIds[d];
            if (destination >= kMaxNodes || !destinationChannels[d]) continue;
            const float gain = m_gains[sourceId][destination].load(std::memory_order_acquire);
            if (!(gain > 0.0f) || !std::isfinite(gain)) continue;
            for (uint32_t c = 0; c < channels; ++c) {
                if (!sourceChannels[c] || !destinationChannels[d][c]) continue;
                for (uint32_t i = 0; i < frames; ++i)
                    destinationChannels[d][c][i] +=
                        (std::isfinite(sourceChannels[c][i]) ? sourceChannels[c][i] : 0.0f) * gain;
            }
        }
    }

    static constexpr uint32_t kMaxNodes = 128;
    static constexpr uint32_t kMaxFeedbackConnections = 16;
    static constexpr uint32_t kMaxFeedbackSamples = 8192;

    bool addFeedbackConnection(uint32_t source, uint32_t dest, float gain = 1.0f) {
        if (source >= kMaxNodes || dest >= kMaxNodes || source == dest || !std::isfinite(gain)) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        for (auto& edge : m_feedback) {
            if (edge.active.load(std::memory_order_acquire) &&
                edge.source.load(std::memory_order_relaxed) == source &&
                edge.dest.load(std::memory_order_relaxed) == dest) {
                edge.active.store(false, std::memory_order_release);
                edge.gain.store(std::clamp(gain, 0.0f, 2.0f), std::memory_order_relaxed);
                std::fill(edge.left.begin(), edge.left.end(), 0.0f);
                std::fill(edge.right.begin(), edge.right.end(), 0.0f);
                edge.active.store(true, std::memory_order_release);
                return true;
            }
        }
        for (auto& edge : m_feedback) {
            if (!edge.active.load(std::memory_order_acquire)) {
                edge.source.store(source, std::memory_order_relaxed);
                edge.dest.store(dest, std::memory_order_relaxed);
                edge.gain.store(std::clamp(gain, 0.0f, 2.0f), std::memory_order_relaxed);
                std::fill(edge.left.begin(), edge.left.end(), 0.0f);
                std::fill(edge.right.begin(), edge.right.end(), 0.0f);
                edge.active.store(true, std::memory_order_release);
                return true;
            }
        }
        return false;
    }

    void removeFeedbackConnection(uint32_t source, uint32_t dest) noexcept {
        for (auto& edge : m_feedback) {
            if (edge.active.load(std::memory_order_acquire) &&
                edge.source.load(std::memory_order_relaxed) == source &&
                edge.dest.load(std::memory_order_relaxed) == dest) {
                edge.active.store(false, std::memory_order_release);
            }
        }
    }

    // Control/diagnostic snapshot. A negative value means the edge does not
    // exist; zero remains a valid explicitly muted feedback edge.
    float feedbackConnectionGain(uint32_t source, uint32_t dest) const noexcept {
        if (source >= kMaxNodes || dest >= kMaxNodes) return -1.0f;
        for (const auto& edge : m_feedback) {
            if (edge.active.load(std::memory_order_acquire) &&
                edge.source.load(std::memory_order_relaxed) == source &&
                edge.dest.load(std::memory_order_relaxed) == dest) {
                return edge.gain.load(std::memory_order_relaxed);
            }
        }
        return -1.0f;
    }

    void copyFeedbackConnections(std::vector<FeedbackConnection>& out) const {
        out.clear();
        out.reserve(kMaxFeedbackConnections);
        for (const auto& edge : m_feedback) {
            if (edge.active.load(std::memory_order_acquire)) {
                out.push_back({edge.source.load(std::memory_order_relaxed),
                               edge.dest.load(std::memory_order_relaxed),
                               edge.gain.load(std::memory_order_relaxed)});
            }
        }
    }

    void injectFeedback(uint32_t dest, float* left, float* right, uint32_t len) const noexcept {
        if (!left || !right || len == 0 || len > kMaxFeedbackSamples) return;
        for (const auto& edge : m_feedback) {
            if (!edge.active.load(std::memory_order_acquire) ||
                edge.dest.load(std::memory_order_relaxed) != dest) continue;
            const float gain = edge.gain.load(std::memory_order_relaxed);
            for (uint32_t i = 0; i < len; ++i) {
                left[i] += edge.left[i] * gain;
                right[i] += edge.right[i] * gain;
            }
        }
    }

    void captureFeedback(uint32_t source, const float* left, const float* right,
                         uint32_t len) noexcept {
        if (!left || !right || len == 0 || len > kMaxFeedbackSamples) return;
        for (auto& edge : m_feedback) {
            if (!edge.active.load(std::memory_order_acquire) ||
                edge.source.load(std::memory_order_relaxed) != source) continue;
            std::copy_n(left, len, edge.left.begin());
            std::copy_n(right, len, edge.right.begin());
            if (len < kMaxFeedbackSamples) {
                std::fill(edge.left.begin() + len, edge.left.end(), 0.0f);
                std::fill(edge.right.begin() + len, edge.right.end(), 0.0f);
            }
        }
    }

    /**
     * @brief Fixed-size, allocation-free view of the compiled routing order.
     *
     * The caller owns `nodes`; the engine only performs atomic loads.  This is
     * safe to call from the audio thread and never takes `m_mutex`.
     */
    bool getTopologyOrder(std::array<uint32_t, kMaxNodes>& nodes,
                          uint32_t& count) const noexcept {
        if (m_dirty.load(std::memory_order_acquire)) {
            count = 0;
            return false;
        }
        const uint64_t generation =
            m_topologyGeneration.load(std::memory_order_acquire);
        if ((generation & 1u) != 0u) {
            count = 0;
            return false;
        }
        const uint32_t publishedCount =
            m_topologyCount.load(std::memory_order_acquire);
        if (publishedCount > kMaxNodes) {
            count = 0;
            return false;
        }

        for (uint32_t i = 0; i < publishedCount; ++i) {
            nodes[i] = m_topology[i].load(std::memory_order_acquire);
        }
        if (m_topologyGeneration.load(std::memory_order_acquire) != generation) {
            count = 0;
            return false;
        }
        count = publishedCount;
        return true;
    }

    uint32_t topologyNodeCount() const noexcept {
        if (m_dirty.load(std::memory_order_acquire)) return 0;
        return std::min(m_topologyCount.load(std::memory_order_acquire), kMaxNodes);
    }

    /**
     * @brief COMPILE: Pre-calculates the processing order for the engine with industrial precision.
     * INDUSTRIAL: Using Rust for robust and perfectly timed graph compilation.
     */
    bool buildGraph() {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_topologyGeneration.fetch_add(1, std::memory_order_acq_rel);
        for (const auto& connection : m_connections) {
            if (wouldCreateCycleLocked(connection.sourceId, connection.destId, &connection)) {
                m_topologyGeneration.fetch_add(1, std::memory_order_release);
                return false;
            }
        }

        // Compile a deterministic topological order without allocating.  The
        // published entries are atomic so an audio-thread reader never races
        // with a rebuild of the graph.
        std::array<uint16_t, kMaxNodes> indegree{};
        std::array<bool, kMaxNodes> active{};
        for (const auto& connection : m_connections) {
            if (connection.sourceId >= kMaxNodes || connection.destId >= kMaxNodes) {
                m_topologyGeneration.fetch_add(1, std::memory_order_release);
                return false;
            }
            active[connection.sourceId] = true;
            active[connection.destId] = true;
            if (indegree[connection.destId] == std::numeric_limits<uint16_t>::max()) {
                m_topologyGeneration.fetch_add(1, std::memory_order_release);
                return false;
            }
            ++indegree[connection.destId];
        }

        std::array<bool, kMaxNodes> emitted{};
        std::array<uint32_t, kMaxNodes> compiled{};
        uint32_t compiledCount = 0;
        while (compiledCount < kMaxNodes) {
            uint32_t next = kMaxNodes;
            for (uint32_t node = 0; node < kMaxNodes; ++node) {
                if (active[node] && !emitted[node] && indegree[node] == 0) {
                    next = node;
                    break;
                }
            }
            if (next == kMaxNodes) break;

            emitted[next] = true;
            compiled[compiledCount++] = next;
            for (const auto& connection : m_connections) {
                if (connection.sourceId == next && indegree[connection.destId] > 0) {
                    --indegree[connection.destId];
                }
            }
        }

        uint32_t activeCount = 0;
        for (bool isActive : active) activeCount += isActive ? 1u : 0u;
        if (compiledCount != activeCount) {
            m_topologyGeneration.fetch_add(1, std::memory_order_release);
            return false;
        }

        for (uint32_t i = 0; i < compiledCount; ++i) {
            m_topology[i].store(compiled[i], std::memory_order_release);
        }
        m_topologyCount.store(compiledCount, std::memory_order_release);
        m_topologyGeneration.fetch_add(1, std::memory_order_release);
        m_dirty.store(false, std::memory_order_release);
        return true;
    }

private:
    bool wouldCreateCycleLocked(uint32_t source, uint32_t dest,
                                const Connection* ignored = nullptr) const {
        if (source == dest || source >= kMaxNodes || dest >= kMaxNodes) return true;
        std::array<bool, kMaxNodes> visited{};
        std::array<uint32_t, kMaxNodes> stack{};
        size_t count = 0;
        stack[count++] = dest;
        while (count > 0) {
            const uint32_t node = stack[--count];
            if (node == source) return true;
            if (visited[node]) continue;
            visited[node] = true;
            for (const auto& edge : m_connections) {
                if (&edge == ignored || edge.sourceId != node || edge.destId >= kMaxNodes) continue;
                if (!visited[edge.destId] && count < stack.size()) stack[count++] = edge.destId;
            }
        }
        return false;
    }

public:
    RoutingEngine() {
        for (auto& row : m_gains) {
            for (auto& gain : row) gain.store(0.0f, std::memory_order_relaxed);
        }
        for (auto& node : m_topology) node.store(0, std::memory_order_relaxed);
    }

private:
    struct FeedbackEdge {
        std::atomic<uint32_t> source{0};
        std::atomic<uint32_t> dest{0};
        std::atomic<float> gain{1.0f};
        alignas(64) std::array<float, kMaxFeedbackSamples> left{};
        std::array<float, kMaxFeedbackSamples> right{};
        std::atomic<bool> active{false};
    };
    std::vector<Connection> m_connections;
    std::array<std::array<std::atomic<float>, kMaxNodes>, kMaxNodes> m_gains{};
    std::array<std::atomic<uint32_t>, kMaxNodes> m_topology{};
    std::atomic<uint32_t> m_topologyCount{0};
    std::atomic<uint64_t> m_topologyGeneration{0};
    mutable std::mutex m_mutex;
    std::atomic<bool> m_dirty{true};
    std::array<FeedbackEdge, kMaxFeedbackConnections> m_feedback{};
};

} // namespace Aura::Core::Engine
