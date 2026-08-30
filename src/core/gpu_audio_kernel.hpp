#pragma once
#include <vector>
#include <iostream>

#if defined(__APPLE__)
#include <Accelerate/Accelerate.h>
#endif

#if defined(__APPLE__) && defined(__OBJC__)
#import <Metal/Metal.h>
#endif

namespace Aura::Core { class AudioBuffer; }

namespace Aura::Core::GPU {

/**
 * @class MetalAudioKernel
 * @brief 【OSS独自の圧倒的スペック：GPUによる並列音声演算】
 */
class MetalAudioKernel {
public:
    static MetalAudioKernel& getInstance() { static MetalAudioKernel i; return i; }
    static void initialize();

    MetalAudioKernel();
    ~MetalAudioKernel();

    void sumBuffers(float* dest, const float* src, uint32_t len);
    void processFXChain(float* buffer, uint32_t len) {
        if (buffer) sumBuffers(buffer, buffer, len);
    }
    void processFXChain(::Aura::Core::AudioBuffer* buffer, uint32_t len);

    void setEnabled(bool e) { m_enabled = e; }
    bool isEnabled() const { return m_enabled; }

private:
    bool m_enabled = true;
    struct MetalSummingContext; 
    MetalSummingContext* m_ctx = nullptr;
};

} // namespace Aura::Core::GPU
