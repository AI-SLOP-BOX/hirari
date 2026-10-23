#pragma once

#include <cstddef>
#include <cstdint>
#include <new>

extern "C" bool hirari_forensic_scratchpad_init();
extern "C" void* hirari_forensic_scratchpad_allocate(size_t size);
extern "C" void hirari_forensic_scratchpad_reset();

namespace Hirari::Core::Concurrency {

class ForensicScratchpad {
public:
    static ForensicScratchpad& getThreadLocal() {
        static thread_local ForensicScratchpad instance;
        return instance;
    }

    void* allocate(size_t size) { return hirari_forensic_scratchpad_allocate(size); }

    template<typename T>
    T* allocateArray(size_t count) {
        return static_cast<T*>(allocate(sizeof(T) * count));
    }

    void reset() { hirari_forensic_scratchpad_reset(); }

private:
    ForensicScratchpad(const ForensicScratchpad&) = delete;
    ForensicScratchpad& operator=(const ForensicScratchpad&) = delete;

    ForensicScratchpad() {
        if (!hirari_forensic_scratchpad_init()) throw std::bad_alloc();
    }
};

} // namespace Hirari::Core::Concurrency
