private:
    struct ReaderGuard {
        explicit ReaderGuard(const SidechainManager& owner) noexcept
            : m_owner(owner),
              m_active(hirari_sidechain_publication_gate_enter_reader(owner.m_publicationGate)) {}
        ~ReaderGuard() {
            if (m_active) hirari_sidechain_publication_gate_leave_reader(m_owner.m_publicationGate);
        }
        bool active() const noexcept { return m_active; }
        ReaderGuard(const ReaderGuard&) = delete;
        ReaderGuard& operator=(const ReaderGuard&) = delete;
        const SidechainManager& m_owner;
        bool m_active;
    };

    struct ControlMutationGuard {
        explicit ControlMutationGuard(SidechainManager& owner) noexcept
            : m_owner(owner), m_lock(owner.m_writerMutex) {
            hirari_sidechain_publication_gate_begin_control(m_owner.m_publicationGate);
        }
        ~ControlMutationGuard() {
            hirari_sidechain_publication_gate_end_control(m_owner.m_publicationGate);
        }
        ControlMutationGuard(const ControlMutationGuard&) = delete;
        ControlMutationGuard& operator=(const ControlMutationGuard&) = delete;
        SidechainManager& m_owner;
        std::unique_lock<std::mutex> m_lock;
    };

    void* m_publicationGate = nullptr;
    mutable std::mutex m_writerMutex;
    std::atomic<bool> m_legacyApiMisuse{false};
};
