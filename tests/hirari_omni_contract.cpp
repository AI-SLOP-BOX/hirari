#include "../src/HirariOmni.hpp"

#include <cassert>
#include <vector>

namespace Hirari::Omni {
alignas(64) char S[kSessionBytes] = {};
std::atomic<int> P{0};
std::atomic<uint64_t> EngineState{0};
}

int main() {
    auto& engine = Hirari::Omni::Engine::i();

    std::size_t written = 123;
    std::vector<char> undersized(1024);
    assert(!engine.save(undersized.data(), undersized.size(), written));
    assert(written == 0);
    assert(!engine.save(nullptr, Hirari::Omni::kSessionBytes, written));
    assert(!engine.load(nullptr, Hirari::Omni::kSessionBytes));
    assert(!engine.execute(999, nullptr));
    assert(!engine.execute(3, nullptr));
    assert(!engine.execute(4, nullptr));

    Hirari::Omni::TrackProxy track{};
    for (std::size_t i = 0; i < Hirari::Omni::kMaxTracks; ++i) {
        assert(engine.addTrack(&track));
    }
    assert(!engine.addTrack(&track));

    return 0;
}
