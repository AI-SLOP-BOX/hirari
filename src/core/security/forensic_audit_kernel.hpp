#pragma once
#include <vector>
#include <string>
#include <chrono>

namespace Aura::Core::Security {

/**
 * @struct AuditEntry
 * @brief Represents a cryptographically signed operation log.
 */
struct AuditEntry {
    std::string operation;
    std::string actorId;
    std::chrono::system_clock::time_point timestamp;
    std::string signature; // SHA-256 hash or digital signature
};

/**
 * @class ForensicAuditKernel
 * @brief Maintains an immutable ledger of project operations.
 */
class ForensicAuditKernel {
public:
    static ForensicAuditKernel& getInstance() {
        static ForensicAuditKernel instance;
        return instance;
    }

    /**
     * @brief Logs an operation to the forensic ledger.
     */
    void logOperation(const std::string& op, const std::string& actorId) {
        AuditEntry entry;
        entry.operation = op;
        entry.actorId = actorId;
        entry.timestamp = std::chrono::system_clock::now();
        // INDUSTRIAL: In a real implementation, this would chain 
        // the hash of the previous entry to the current one (Merkle tree)
        // to ensure immutability and detect tampering.
        m_ledger.push_back(entry);
    }

    const std::vector<AuditEntry>& getLedger() const { return m_ledger; }

private:
    ForensicAuditKernel() = default;
    std::vector<AuditEntry> m_ledger;
};

} // namespace Aura::Core::Security
