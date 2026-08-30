#include <cassert>
#include <cstring>
#include <vector>

#include "../src/core/engine/cloud_sync_orchestrator.hpp"
#include "../src/ui/waveform_cache.hpp"

int main() {
    using Aura::Core::Engine::CloudSyncOrchestrator;
    using Aura::Core::Engine::UserChange;

    CloudSyncOrchestrator sync;
    assert(!sync.isConnected());
    assert(sync.connect("https://sync.invalid", "session-a"));
    assert(sync.isConnected());
    assert(sync.serverUrl() == "https://sync.invalid");
    assert(sync.sessionId() == "session-a");

    CloudSyncOrchestrator::RemoteUser user{};
    user.id = 7;
    std::strcpy(user.name, "Editor");
    user.isActive = true;
    sync.upsertRemoteUser(user);
    assert(sync.hasRemoteUser(7));
    sync.removeRemoteUser(7);
    assert(!sync.hasRemoteUser(7));

    UserChange older{10, 2, 11, 0.25f, {}};
    UserChange newer{20, 2, 11, 0.75f, {}};
    sync.pushChange(older);
    sync.pushChange(newer);
    assert(sync.pendingChangeCount() == 1);
    std::vector<UserChange> pulled;
    sync.pullChanges(pulled);
    assert(pulled.size() == 1);
    assert(pulled[0].newValue == 0.75f);
    sync.disconnect();
    assert(!sync.isConnected());

    auto& cache = Aura::UI::WaveformCache::getInstance();
    cache.clearRegion(9001);
    assert(cache.requestWaveform(9001, 2,
                                 std::vector<float>{-1.0f, 0.5f, 2.0f, 1.0f}));
    assert(cache.isPending(9001, 2));
    cache.waitForPending();
    assert(!cache.hasFailure(9001, 2));
    Aura::UI::WaveformLevel level;
    assert(cache.copyWaveform(9001, 2, level));
    assert(level.minPeaks == std::vector<float>({-1.0f, 1.0f}));
    assert(level.maxPeaks == std::vector<float>({0.5f, 2.0f}));

    cache.clearRegion(9002);
    assert(cache.requestWaveform(9002, 1,
                                 std::vector<float>{-1.0f, 1.0f}));
    cache.clearRegion(9002);
    cache.waitForPending();
    assert(!cache.copyWaveform(9002, 1, level));
    return 0;
}
