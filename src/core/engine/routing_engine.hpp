#pragma once
#include <cstdint>
#include <vector>
#include <algorithm>
#include <cmath>
#include <array>
#include <type_traits>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

/**
 * @class RoutingEngine
 * @brief THE NERVE SYSTEM.
 * Manages audio bus routing, sidechains, and parallel processing chains.
 */
class RoutingEngine {
public:
    static RoutingEngine& getInstance() { static RoutingEngine engine; return engine; }

    class SendPdcReadGuard {
    public:
        explicit SendPdcReadGuard(RoutingEngine& owner) noexcept : m_manager(owner.m_sendPdcManager) {
            hirari_send_pdc_manager_enter_read(m_manager);
        }
        ~SendPdcReadGuard() {
            hirari_send_pdc_manager_leave_read(m_manager);
        }
        SendPdcReadGuard(const SendPdcReadGuard&) = delete;
        SendPdcReadGuard& operator=(const SendPdcReadGuard&) = delete;
    private:
        void* m_manager;
    };

    struct Connection {
        uint32_t sourceId;
        uint32_t destId;
        float gain = 1.0f;
        bool send = false;
        bool preFader = false;
    };
    static_assert(std::is_standard_layout_v<Connection>);
    static_assert(offsetof(Connection, sourceId) == 0 &&
                  offsetof(Connection, destId) == 4 &&
                  offsetof(Connection, gain) == 8 &&
                  offsetof(Connection, send) == 12 &&
                  offsetof(Connection, preFader) == 13 && sizeof(Connection) == 16);

    struct FeedbackConnection {
        uint32_t sourceId = 0;
        uint32_t destId = 0;
        float gain = 1.0f;
    };

    /**
     * @brief Adds a route through the Rust-owned graph and publishes its gain.
     */
    void addConnection(uint32_t s, uint32_t d, float g = 1.0f,
                       bool send = false, bool preFader = false) {
        (void)hirari_routing_runtime_add_connection(
            m_graphState, m_gainState, m_topologyState, s, d, g, send, preFader);
    }

    void removeConnection(uint32_t s, uint32_t d) {
        hirari_routing_runtime_remove_connection(
            m_graphState, m_gainState, m_topologyState, s, d);
    }

    void removeSendConnection(uint32_t s, uint32_t d) {
        hirari_routing_runtime_remove_send(
            m_graphState, m_gainState, m_sendPdcManager, m_topologyState, s, d);
    }

    // Adds a processing-order edge without routing the source audio into the
    // destination. Sidechain detector inputs need the source's current block
    // before the destination plug-in runs, but must not become an audible send.
    bool addProcessingDependency(uint32_t sourceId, uint32_t destId) {
        return hirari_routing_runtime_add_dependency(
            m_graphState, m_topologyState, sourceId, destId);
    }

    void removeProcessingDependency(uint32_t sourceId, uint32_t destId) {
        hirari_routing_runtime_remove_dependency(
            m_graphState, m_topologyState, sourceId, destId);
    }

    void removeProcessingDependenciesForNode(uint32_t nodeId) {
        hirari_routing_runtime_remove_dependencies_for_node(
            m_graphState, m_topologyState, nodeId);
    }

    // Remove all normal signal edges attached to a deleted track/bus. The
    // caller may retain `removed` for an Undo restore with original gains.
    void removeConnectionsForNode(uint32_t nodeId, std::vector<Connection>& removed) {
        removed.clear();
        if (nodeId >= kMaxNodes) return;
        void* snapshot = hirari_routing_runtime_remove_connections_for_node(
            m_graphState, m_gainState, m_sendPdcManager, m_topologyState, nodeId);
        const size_t count = hirari_routing_graph_snapshot_count(snapshot);
        removed.resize(count);
        const size_t copied = hirari_routing_graph_snapshot_copy(snapshot, removed.data(), count);
        removed.resize(copied);
        hirari_routing_graph_snapshot_destroy(snapshot);
    }

    void resetForProject() {
        hirari_routing_runtime_reset(
            m_graphState, m_gainState, m_feedbackState,
            m_sendPdcManager, m_topologyState);
    }


    bool hasConnection(uint32_t s, uint32_t d) const {
        if (s >= kMaxNodes || d >= kMaxNodes) return false;
        return hirari_routing_gains_route(m_gainState, s, d) > 0.0f;
    }

    // Control-thread snapshot used by PDC and graph compilation. The audio
    // callback consumes only the already-published topology/gain arrays.
    void copyConnections(std::vector<Connection>& out) const {
        void* snapshot = hirari_routing_graph_snapshot_create(m_graphState);
        const size_t count = hirari_routing_graph_snapshot_count(snapshot);
        out.resize(count);
        const size_t copied = hirari_routing_graph_snapshot_copy(snapshot, out.data(), count);
        out.resize(copied);
        hirari_routing_graph_snapshot_destroy(snapshot);
    }

    float connectionGain(uint32_t s, uint32_t d) const noexcept {
        if (s >= kMaxNodes || d >= kMaxNodes) return 0.0f;
        return hirari_routing_gains_route(m_gainState, s, d);
    }

    bool shouldProcessNode(uint32_t sourceId, bool offlineTargetActive,
                           uint32_t offlineTargetId, bool anySolo,
                           bool sourceSolo) const noexcept {
        return hirari_routing_should_process_node(
            m_gainState, sourceId, offlineTargetActive, offlineTargetId,
            anySolo, sourceSolo);
    }

    float sendConnectionGain(uint32_t s, uint32_t d) const noexcept {
        if (s >= kMaxNodes || d >= kMaxNodes) return 0.0f;
        return hirari_routing_gains_send(m_gainState, s, d);
    }

    bool hasSendConnection(uint32_t s, uint32_t d) const noexcept {
        if (s >= kMaxNodes || d >= kMaxNodes) return false;
        return hirari_routing_gains_has_send(m_gainState, s, d);
    }

    bool sendPreFader(uint32_t s, uint32_t d) const noexcept {
        if (s >= kMaxNodes || d >= kMaxNodes) return false;
        return hirari_routing_gains_send_pre_fader(m_gainState, s, d);
    }

    // Send compensation is independent from the source's primary output
    // delay. State is allocated on the control thread. Removed histories move
    // to a retire list and are reclaimed only after audio-block readers exit.
    bool setSendPdcCompensationSamples(uint32_t sourceId, uint32_t destId,
                                       uint32_t samples) {
        if (sourceId >= kMaxNodes || destId >= kMaxNodes) return false;
        return hirari_send_pdc_manager_set_delay(m_sendPdcManager, sourceId, destId, samples);
    }

    void retireSendPdcState(uint32_t sourceId, uint32_t destId) {
        if (sourceId >= kMaxNodes || destId >= kMaxNodes) return;
        hirari_send_pdc_manager_retire(m_sendPdcManager, sourceId, destId);
    }

    void retireAllSendPdcStates() {
        hirari_send_pdc_manager_retire_all(m_sendPdcManager);
    }

    bool processSendPdc(uint32_t sourceId, uint32_t destId,
                        const float* inputLeft, const float* inputRight,
                        float* outputLeft, float* outputRight,
                        uint32_t frames, uint32_t additionalDelay = 0,
                        float inputGain = 1.0f) noexcept {
        if (sourceId >= kMaxNodes || destId >= kMaxNodes || !inputLeft || !inputRight ||
            !outputLeft || !outputRight || frames == 0)
            return false;
        return hirari_send_pdc_manager_process(
            m_sendPdcManager, sourceId, destId, inputLeft, inputRight,
            outputLeft, outputRight, frames, additionalDelay, inputGain);
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
        hirari_routing_fanout_stereo(m_gainState, sourceId, left, right,
                                     destinationLeft, destinationRight,
                                     destinationIds, destinationCount, frames);
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
        hirari_routing_fanout_planar(m_gainState, sourceId, sourceChannels,
                                     destinationChannels, destinationIds,
                                     destinationCount, channelCount, frames);
    }

    static constexpr uint32_t kMaxNodes = 128;
    static constexpr uint32_t kMaxFeedbackConnections = 16;
    static constexpr uint32_t kMaxFeedbackSamples = 8192;

    bool addFeedbackConnection(uint32_t source, uint32_t dest, float gain = 1.0f) {
        return hirari_routing_feedback_add(m_feedbackState, source, dest, gain);
    }

    void removeFeedbackConnection(uint32_t source, uint32_t dest) noexcept {
        hirari_routing_feedback_remove(m_feedbackState, source, dest);
    }

    // Feedback buffers are fixed-size and safe to detach on the control
    // thread. The saved edge values can be restored after an Undo.
    void removeFeedbackConnectionsForNode(uint32_t nodeId,
                                          std::vector<FeedbackConnection>& removed) {
        removed.clear();
        if (nodeId >= kMaxNodes) return;
        static_assert(std::is_standard_layout_v<FeedbackConnection>);
        static_assert(sizeof(FeedbackConnection) == sizeof(uint32_t) * 2 + sizeof(float));
        std::array<FeedbackConnection, kMaxFeedbackConnections> snapshot{};
        const size_t count = std::min(
            hirari_routing_feedback_remove_node(
                m_feedbackState, nodeId, snapshot.data(), snapshot.size()), snapshot.size());
        removed.assign(snapshot.begin(), snapshot.begin() + static_cast<std::ptrdiff_t>(count));
    }

    // Control/diagnostic snapshot. A negative value means the edge does not
    // exist; zero remains a valid explicitly muted feedback edge.
    float feedbackConnectionGain(uint32_t source, uint32_t dest) const noexcept {
        if (source >= kMaxNodes || dest >= kMaxNodes) return -1.0f;
        return hirari_routing_feedback_gain(m_feedbackState, source, dest);
    }

    void copyFeedbackConnections(std::vector<FeedbackConnection>& out) const {
        out.clear();
        out.reserve(kMaxFeedbackConnections);
        static_assert(std::is_standard_layout_v<FeedbackConnection>);
        static_assert(sizeof(FeedbackConnection) == sizeof(uint32_t) * 2 + sizeof(float));
        out.resize(kMaxFeedbackConnections);
        const size_t count = std::min(
            hirari_routing_feedback_copy(m_feedbackState, out.data(), out.size()), out.size());
        out.resize(count);
    }

    void injectFeedback(uint32_t dest, float* left, float* right, uint32_t len) const noexcept {
        hirari_routing_feedback_inject(m_feedbackState, dest, left, right, len);
    }

    void captureFeedback(uint32_t source, const float* left, const float* right,
                         uint32_t len) noexcept {
        hirari_routing_feedback_capture(m_feedbackState, source, left, right, len);
    }

    /**
     * @brief Fixed-size, allocation-free view of the compiled routing order.
     *
     * The caller owns `nodes`; Rust copies from its fixed atomic publication
     * cache. This is safe on the audio thread and never takes a control lock.
     */
    bool getTopologyOrder(std::array<uint32_t, kMaxNodes>& nodes,
                          uint32_t& count) const noexcept {
        if (hirari_routing_topology_copy(
                m_topologyState, nodes.data(), nodes.size(), &count)) return true;
        count = 0;
        return false;
    }

    uint32_t topologyNodeCount() const noexcept {
        return std::min(hirari_routing_topology_node_count(m_topologyState), kMaxNodes);
    }

    const void* nativeGainState() const noexcept { return m_gainState; }
    void* nativeSendPdcState() const noexcept { return m_sendPdcManager; }
    void* nativeTopologyState() const noexcept { return m_topologyState; }

    /**
     * @brief COMPILE: Pre-calculates the processing order for the engine with industrial precision.
     * INDUSTRIAL: Using Rust for robust and perfectly timed graph compilation.
     */
    bool buildGraph() {
        return hirari_routing_topology_build(m_topologyState, m_graphState);
    }

public:
    RoutingEngine() : m_graphState(hirari_routing_graph_create()),
                      m_gainState(hirari_routing_gains_create()),
                      m_feedbackState(hirari_routing_feedback_create()),
                      m_sendPdcManager(hirari_send_pdc_manager_create()),
                      m_topologyState(hirari_routing_topology_create()) {}
    ~RoutingEngine() {
        hirari_routing_graph_destroy(m_graphState);
        hirari_routing_gains_destroy(m_gainState);
        hirari_routing_feedback_destroy(m_feedbackState);
        hirari_send_pdc_manager_destroy(m_sendPdcManager);
        hirari_routing_topology_destroy(m_topologyState);
    }

private:
    void* m_graphState = nullptr;
    void* m_gainState = nullptr;
    void* m_feedbackState = nullptr;
    void* m_sendPdcManager = nullptr;
    void* m_topologyState = nullptr;
};

} // namespace Hirari::Core::Engine
