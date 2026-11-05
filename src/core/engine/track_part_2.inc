    // A control edit follows one protocol: drop the frozen render, mutate, then
    // bump the layout revision. A helper keeps the three steps from being
    // re-ordered site by site.
    template <typename Fn>
    void editControl(const Fn& fn) {
        invalidateFrozenForEdit();
        fn();
        markProjectLayoutChanged();
    }

    // A successful edit that only needs the layout revision bumped; the frozen
    // render stays valid because nothing audible changed.
    bool commitLayoutChange(bool changed) {
        if (changed) markProjectLayoutChanged();
        return changed;
    }

    // Same protocol for edits that only count when the chain actually changed.
    bool commitEditChange(bool changed) {
        if (changed) {
            invalidateFrozenForEdit();
            markProjectLayoutChanged();
        }
        return changed;
    }

    void setVolume(float v) {
        editControl([&] { hirari_channel_strip_set_gain(m_strip, v); });
    }
    void setPan(float p) {
        editControl([&] { hirari_channel_strip_set_pan(m_strip, p); });
    }

    // One removed insert, addressed by the token the chain issued for it. The
    // chain keeps the node alive until it is restored or the track dies, so an
    // undo entry carries no processor reference.
    struct PluginSnapshot {
        uint32_t token = 0;
        uint32_t type = 0;
        bool bypassed = false;
        bool parallel = false;
    };

    bool removePlugin(uint32_t pluginIndex, PluginSnapshot* snapshot = nullptr) {
        bool bypassed = false;
        bool parallel = false;
        uint32_t type = 0;
        uint32_t token = 0;
        if (!hirari_effect_chain_remove(m_chain, pluginIndex, &bypassed, &parallel, &type,
                                        &token))
            return false;
        if (snapshot) *snapshot = {token, type, bypassed, parallel};
        return commitEditChange(true);
    }
    bool restorePlugin(uint32_t pluginIndex, const PluginSnapshot& snapshot) {
        return commitEditChange(hirari_effect_chain_restore(
            m_chain, pluginIndex, snapshot.token, snapshot.bypassed, snapshot.parallel));
    }
    bool movePlugin(uint32_t from, uint32_t to) {
        return commitEditChange(hirari_effect_chain_move(m_chain, from, to));
    }

    void reclaimEffectChainRetiredMemory() { hirari_effect_chain_reclaim(m_chain); }
    size_t getPluginCount() const { return hirari_effect_chain_count(m_chain); }
    size_t sandboxSlotCount() const noexcept {
        return hirari_effect_chain_sandbox_slot_count(m_chain);
    }
    uint32_t maintainSandboxedPlugins(bool autoRestart) {
        const uint32_t recovered = hirari_effect_chain_sandbox_maintain(m_chain, autoRestart);
        if (recovered != 0) markProjectLayoutChanged();
        return recovered;
    }
    struct SandboxStatus {
        uint32_t pluginIndex = 0;
        bool alive = false;
        bool canRetry = false;
        uint8_t failure = 0;
        uint32_t droppedOutputMidi = 0;
        uint32_t mailboxOverruns = 0;
        uint32_t inputMidiTruncations = 0;
        uint32_t recoveryMode = 0;
    };
    std::vector<SandboxStatus> getSandboxStatuses() const {
        std::vector<SandboxStatus> result;
        result.reserve(sandboxSlotCount());
        for (size_t slot = 0; slot < sandboxSlotCount(); ++slot) {
            HirariEffectChainSandboxStatus status{};
            if (!hirari_effect_chain_sandbox_status(m_chain, slot, &status)) continue;
            result.push_back({status.plugin_index, status.alive != 0, status.can_retry != 0,
                              status.failure, status.dropped_output_midi,
                              status.mailbox_overruns, status.input_midi_truncations,
                              status.recovery_mode});
        }
        return result;
    }

    virtual bool setPluginParameter(uint32_t pluginIndex, uint32_t parameterId, float value) {
        return commitEditChange(
            hirari_effect_chain_set_parameter(m_chain, pluginIndex, parameterId, value));
    }
    // The chain's parameter, bypass and sandbox-repair reads are Rust-side now
    // (engine_analysis_reads.rs resolves the track's chain storage directly).

public:
