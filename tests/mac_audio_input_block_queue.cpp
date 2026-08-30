#include <cassert>
#include <cstdint>

#include "../src/core/driver/mac_audio_driver_host.hpp"

int main() {
    using Aura::Core::Driver::MacAudioInputBlockQueue;

    MacAudioInputBlockQueue queue;
    float inputLeft[4] = {1.0f, 2.0f, 3.0f, 4.0f};
    float inputRight[4] = {-1.0f, -2.0f, -3.0f, -4.0f};
    const float* input[2] = {inputLeft, inputRight};

    assert(queue.push_planar(input, 2, 4));
    float outputLeft[4]{};
    float outputRight[4]{};
    float* output[2] = {outputLeft, outputRight};
    MacAudioInputBlockQueue::BlockInfo info;
    uint64_t dropped = 0;
    assert(queue.poll(output, 2, 4, info, dropped));
    assert(info.channelCount == 2 && info.frameCount == 4 && dropped == 0);
    for (uint32_t i = 0; i < 4; ++i) {
        assert(outputLeft[i] == inputLeft[i]);
        assert(outputRight[i] == inputRight[i]);
    }

    for (uint32_t i = 0; i < MacAudioInputBlockQueue::kCapacity; ++i)
        assert(queue.push_planar(input, 2, 4));
    assert(!queue.push_planar(input, 2, 4));
    assert(!queue.poll(output, 2, 4, info, dropped) || dropped == 1);
    assert(dropped == 1);

    assert(!queue.push_planar(input, 2, MacAudioInputBlockQueue::kMaxFrames + 1));
    assert(queue.dropped_blocks() == 1);
    return 0;
}
