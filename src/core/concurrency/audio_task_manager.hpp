#pragma once
#include <cstdint>

extern "C" uint64_t hirari_denormal_control_enter();
extern "C" void hirari_denormal_control_leave(uint64_t state);

namespace Hirari::Core::Concurrency {

struct ScopedDenormalGuard {
    ScopedDenormalGuard() : m_oldReg(hirari_denormal_control_enter()) {}
    ~ScopedDenormalGuard() { hirari_denormal_control_leave(m_oldReg); }
private:
    uint64_t m_oldReg = 0;
};

} // namespace Hirari::Core::Concurrency
