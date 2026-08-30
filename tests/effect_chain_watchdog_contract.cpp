#include "src/core/effect_chain.hpp"

#include <cassert>
#include <atomic>

class WatchdogProbe final : public Aura::DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Aura::Core::AudioBuffer&, Aura::Core::MidiBuffer&,
                 const Aura::DSP::ProcessContext&) noexcept override {}
    void reset() noexcept override {}

    bool takeWatchdogTrip() noexcept override {
        return pending.exchange(false, std::memory_order_acq_rel);
    }

    void trip() noexcept { pending.store(true, std::memory_order_release); }

private:
    std::atomic<bool> pending{false};
};

int main() {
    Aura::Core::EffectChain chain;
    auto first = std::make_shared<WatchdogProbe>();
    auto second = std::make_shared<WatchdogProbe>();
    chain.addProcessor(first);
    chain.addProcessor(second);

    first->trip();
    second->trip();
    assert(chain.takeWatchdogTrips() == 2);
    assert(chain.takeWatchdogTrips() == 0);

    second->trip();
    assert(chain.takeWatchdogTrips() == 1);
    assert(chain.takeWatchdogTrips() == 0);
    return 0;
}
