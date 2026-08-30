#include <cassert>
#include "../src/core/network/crdt_engine.hpp"

int main() {
    Aura::Core::Network::CRDTEngine crdt;
    crdt.apply({"track/1/volume", {0.5f, 10, 2}});
    crdt.apply({"track/1/volume", {0.2f, 9, 99}});
    Aura::Core::Network::CRDTEngine::LWWRegister value{};
    assert(crdt.read("track/1/volume", value));
    assert(value.value == 0.5f && value.userId == 2);
    crdt.apply({"track/1/volume", {0.8f, 10, 3}});
    assert(crdt.read("track/1/volume", value));
    assert(value.value == 0.8f && value.userId == 3);
    assert(crdt.snapshot().size() == 1);
    return 0;
}
