#pragma once
#include <string>
#include "forensic_journaler.hpp"

namespace Hirari::Core::Diagnostics {

class ForensicKernel {
public:
    static ForensicKernel& getInstance() {
        static ForensicKernel instance;
        return instance;
    }

    void recordDecision(int phaseId, const std::string& description, float value) {
        ForensicJournaler::getInstance().logDescription(
            static_cast<uint32_t>(phaseId), description, value);
    }

private:
    ForensicKernel() = default;
};

} // namespace Hirari::Core::Diagnostics
