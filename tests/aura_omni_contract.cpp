#include "../src/AuraOmni.hpp"

#include <cassert>
#include <vector>

namespace Aura::Omni {
alignas(64) char S[kSessionBytes] = {};
std::atomic<int> P{0};
std::atomic<uint64_t> EngineState{0};
}

int main() {
    auto& engine = Aura::Omni::Engine::i();

    std::size_t written = 123;
    std::vector<char> undersized(1024);
    assert(!engine.save(undersized.data(), undersized.size(), written));
    assert(written == 0);
    assert(!engine.save(nullptr, Aura::Omni::kSessionBytes, written));
    assert(!engine.load(nullptr, Aura::Omni::kSessionBytes));
    assert(!engine.execute(999, nullptr));
    assert(!engine.execute(3, nullptr));
    assert(!engine.execute(4, nullptr));

    Aura::Omni::TrackProxy track{};
    for (std::size_t i = 0; i < Aura::Omni::kMaxTracks; ++i) {
        assert(engine.addTrack(&track));
    }
    assert(!engine.addTrack(&track));

    return 0;
}
