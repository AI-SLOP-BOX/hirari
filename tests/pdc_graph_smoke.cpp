#include "src/core/engine/pdc_graph.hpp"
#include "src/core/engine/routing_graph_pdc.hpp"
#include "src/core/engine/pdc_manager.hpp"

#include <cassert>
#include <array>
#include <thread>

int main() {
    using Aura::Core::Engine::PDCGraphSolver;
    PDCGraphSolver solver;
    std::map<uint32_t, PDCGraphSolver::Node> graph{
        {1, {1, 1, 0, 0, true, {3}}},
        {2, {2, 5, 0, 0, true, {3}}},
        {3, {3, 7, 0, 0, true, {}}},
    };
    solver.solve(graph);
    assert(!solver.hasCycle());
    assert(solver.globalProjectLatency() == 12);
    assert(graph.at(1).totalLatency == 8);
    assert(graph.at(1).compensation == 4);
    assert(graph.at(2).totalLatency == 12);
    assert(graph.at(2).compensation == 0);
    assert(graph.at(3).totalLatency == 7);
    assert(graph.at(3).compensation == 5);

    graph[3].downstream = {1};
    solver.solve(graph);
    assert(solver.hasCycle());

    std::map<uint32_t, PDCGraphSolver::Node> saturated{
        {4, {4, 0xFFFFFFFFu, 0, 0, true, {5}}},
        {5, {5, 1, 0, 0, true, {}}},
    };
    solver.solve(saturated);
    assert(solver.globalProjectLatency() == 0xFFFFFFFFu);
    assert(saturated.at(4).totalLatency == 0xFFFFFFFFu);

    // The production routing graph must use the shared solver rather than a
    // second latency algorithm with subtly different semantics.
    Aura::Core::Engine::RoutingGraphPDC routing;
    routing.addNode(1, 1);
    routing.addNode(2, 5);
    routing.addNode(3, 7);
    routing.connect(1, 3);
    routing.connect(2, 3);
    assert(routing.compileGraph());
    assert(routing.getDelayForNode(1) == 4);
    assert(routing.getDelayForNode(2) == 0);
    assert(routing.getDelayForNode(3) == 5);

    Aura::Core::Engine::RoutingGraphPDC cyclicRouting;
    cyclicRouting.addNode(1, 1);
    cyclicRouting.addNode(2, 1);
    cyclicRouting.connect(1, 2);
    cyclicRouting.connect(2, 1);
    assert(!cyclicRouting.compileGraph());

    // Sample-level contract: each path must land at the same sample after
    // applying compensation, not merely report matching latency numbers.
    PDCGraphSolver impulseSolver;
    std::map<uint32_t, PDCGraphSolver::Node> impulseGraph{
        {1, {1, 1, 0, 0, true, {3}}},
        {2, {2, 5, 0, 0, true, {3}}},
        {3, {3, 7, 0, 0, true, {}}},
    };
    impulseSolver.solve(impulseGraph);
    constexpr std::size_t impulse = 3;
    std::array<float, 32> pathA{};
    std::array<float, 32> pathB{};
    const auto place = [](auto& buffer, std::size_t index) { buffer[index] = 1.0f; };
    place(pathA, impulse + impulseGraph.at(1).totalLatency + impulseGraph.at(1).compensation);
    place(pathB, impulse + impulseGraph.at(2).totalLatency + impulseGraph.at(2).compensation);
    assert(pathA == pathB);

    auto& manager = Aura::Core::Engine::PDCManager::getInstance();
    assert(manager.bindControlThread());
    manager.resetForProject();
    const auto initialConfiguration = manager.configurationGeneration();
    manager.setTrackLatency(1, 128);
    manager.setTrackLatency(2, 64);
    manager.setTrackDest(1, 2);
    manager.setTrackDest(2, 1);
    manager.recalculate();
    assert(manager.hasCycle());
    assert(manager.getCompensationOffset(1) == 0);
    assert(manager.getCompensationOffset(2) == 0);
    assert(manager.auditAgainstSharedSolver());
    // Repair the route and prove the same manager can leave the fault state.
    // This guards against a sticky cycle flag and stale zeroed offsets.
    manager.setTrackDest(2, Aura::Core::Engine::PDCManager::kMasterID);
    manager.recalculate();
    assert(manager.configurationGeneration() > initialConfiguration);
    assert(!manager.hasCycle());
    assert(manager.getGlobalMaxLatency() == 192);
    assert(manager.getCompensationOffset(1) == 0);
    assert(manager.getCompensationOffset(2) == 128);
    assert(manager.auditAgainstSharedSolver());
    // A transient cycle must not publish an all-zero replacement snapshot.
    const auto validTrackOffset = manager.getCompensationOffset(2);
    const auto validGlobalLatency = manager.getGlobalMaxLatency();
    manager.setTrackDest(2, 1);
    manager.recalculate();
    assert(manager.hasCycle());
    assert(manager.getCompensationOffset(2) == validTrackOffset);
    assert(manager.getGlobalMaxLatency() == validGlobalLatency);
    manager.setTrackDest(2, Aura::Core::Engine::PDCManager::kMasterID);
    manager.recalculate();
    assert(!manager.hasCycle());
    assert(manager.getCompensationOffset(2) == validTrackOffset);
    // Keep the manager memo unsigned: a saturated uint32 latency must not be
    // reinterpreted as a negative cached path on a subsequent traversal.
    manager.resetForProject();
    manager.setTrackLatency(1, 0xFFFFFFFFu);
    manager.setTrackLatency(2, 1u);
    manager.setTrackDest(2, 1);
    manager.recalculate();
    assert(!manager.hasCycle());
    assert(manager.getGlobalMaxLatency() == 0xFFFFFFFFu);
    assert(manager.getCompensationOffset(1) == 0u);
    assert(manager.getCompensationOffset(2) == 0u);
    bool rejectedAudioThreadEdit = false;
    std::thread audioLikeCaller([&] {
        rejectedAudioThreadEdit = !manager.setTrackLatency(1, 7);
    });
    audioLikeCaller.join();
    assert(rejectedAudioThreadEdit);
    manager.resetForProject();

    return 0;
}
