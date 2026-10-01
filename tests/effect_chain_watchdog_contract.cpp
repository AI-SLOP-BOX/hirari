#include "src/core/effect_chain.hpp"

#include <cassert>
#include <atomic>
#include <chrono>
#include <memory>
#include <thread>

class WatchdogProbe final : public Hirari::DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Hirari::Core::AudioBuffer&, Hirari::Core::MidiBuffer&,
                 const Hirari::DSP::ProcessContext&) noexcept override {}
    void reset() noexcept override {}

    bool takeWatchdogTrip() noexcept override {
        return pending.exchange(false, std::memory_order_acq_rel);
    }

    void trip() noexcept { pending.store(true, std::memory_order_release); }

private:
    std::atomic<bool> pending{false};
};

// prepareToPlay is deliberately slow so a control-thread mutation holds
// m_mutex for a measurable interval.
class SlowConfiguringProbe final : public Hirari::DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }
    void process(Hirari::Core::AudioBuffer&, Hirari::Core::MidiBuffer&,
                 const Hirari::DSP::ProcessContext&) noexcept override {}
    void reset() noexcept override {}
    bool takeWatchdogTrip() noexcept override { return false; }
    uint32_t getTailSamples() const noexcept override { return 480; }
};

// The engine-health overlay polls getTotalTailSamples()/nonFiniteSampleCount()
// on a timer. Those readers are noexcept, so they must not block behind a
// control-thread mutation: the previous implementation took m_mutex and then
// invoked plugin code through hirari_effect_chain_runtime_metric, which made a
// single parameter/sample-rate edit stall every concurrent poll.
int main() {
    {
        Hirari::Core::EffectChain chain;
        auto slow = std::make_shared<SlowConfiguringProbe>();
        chain.addProcessor(slow);

        std::atomic<bool> stop{false};
        std::thread hog([&] {
            while (!stop.load(std::memory_order_acquire)) {
                chain.setSampleRate(48000.0, 512);
            }
        });

        auto worst = std::chrono::steady_clock::duration::zero();
        for (int i = 0; i < 300; ++i) {
            const auto started = std::chrono::steady_clock::now();
            (void)chain.getTotalTailSamples();
            (void)chain.nonFiniteSampleCount();
            const auto elapsed = std::chrono::steady_clock::now() - started;
            if (elapsed > worst) worst = elapsed;
            std::this_thread::yield();
        }
        stop.store(true, std::memory_order_release);
        hog.join();

        const auto worstMs =
            std::chrono::duration_cast<std::chrono::milliseconds>(worst).count();
        // The lock holder sleeps 20ms inside prepareToPlay, so a blocking read
        // would land near that figure; the non-blocking path stays far below.
        assert(worstMs < 5 && "diagnostic reader blocked on the control-thread mutex");
    }

    Hirari::Core::EffectChain chain;
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
