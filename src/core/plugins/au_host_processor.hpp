#pragma once

#include <AudioToolbox/AudioToolbox.h>
#include <AudioUnit/AudioUnit.h>
#include "../../dsp/iprocessor.hpp"
#include <vector>
#include <memory>
#include <stdexcept>
#include <algorithm>
#include <chrono>
#include <thread>
#include <cmath>

#if defined(__APPLE__)
#include <CoreFoundation/CoreFoundation.h>
#include <objc/message.h>
#include <objc/runtime.h>
#endif

#include "../log_buffer.hpp"

namespace Aura::Core::Plugins {

/**
 * @class SiloedAllocator
 * @brief Phase 16: Partitioned memory silo for plugin sandboxing.
 */
class SiloedAllocator {
public:
    SiloedAllocator(size_t size) : m_size(size) {
        m_base = std::malloc(size);
        m_ptr.store(reinterpret_cast<uintptr_t>(m_base));
    }
    ~SiloedAllocator() { std::free(m_base); }

    void* allocate(size_t sz) {
        uintptr_t current;
        uintptr_t next;
        const uintptr_t limit = reinterpret_cast<uintptr_t>(m_base) + m_size;

        do {
            current = m_ptr.load(std::memory_order_relaxed);
            next = current + sz;
            if (next > limit) return nullptr;
        } while (!m_ptr.compare_exchange_weak(current, next, std::memory_order_release, std::memory_order_relaxed));

        return reinterpret_cast<void*>(current);
    }

    void reset() { m_ptr.store(reinterpret_cast<uintptr_t>(m_base)); }

private:
    void* m_base;
    size_t m_size;
    std::atomic<uintptr_t> m_ptr;
};

/**
 * @class AUHostProcessor
 * @brief AUv2 Plugin Host implementation.
 * Provides a wrapper for AudioUnit instances with parameter scheduling.
 */
class AUHostProcessor : public ::Aura::DSP::IProcessor {
#include "au_host_processor_public.inc"
#include "au_host_processor_private.inc"

};

} // namespace Aura::Core::Plugins
