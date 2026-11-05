bool HirariUnifiedEngine::bounce_project_async(std::string_view path, uint32_t format) {
    return hirari_bounce_start_async(m_bounceSession, this, bounceOps(),
                                     reinterpret_cast<const uint8_t*>(path.data()), path.size(),
                                     format);
}

float HirariUnifiedEngine::get_bounce_progress() const noexcept {
    return hirari_bounce_progress(m_bounceSession);
}

uint32_t HirariUnifiedEngine::get_bounce_state() const noexcept {
    return hirari_bounce_state(m_bounceSession);
}

bool HirariUnifiedEngine::cancel_bounce() noexcept {
    return hirari_bounce_cancel(m_bounceSession);
}

bool HirariUnifiedEngine::pause_bounce() noexcept {
    return hirari_bounce_pause(m_bounceSession);
}

bool HirariUnifiedEngine::resume_bounce() noexcept {
    return hirari_bounce_resume(m_bounceSession);
}

bool HirariUnifiedEngine::set_project_scale(int32_t root, int32_t type) {
    // The bridge validates the ranges before this is reached.
    hirari_engine_block_set_project_scale(m_engineBlock, root, type);
    return true;
}

bool HirariUnifiedEngine::set_articulation_map(uint32_t trackId, uint32_t) {
    return findPublishedTrack(trackId) != nullptr;
}



void HirariUnifiedEngine::installExpressionMapEditHost() {
    const HirariExpressionMapEditHostOps ops{
        [](void* host, uint32_t trackId) -> void* {
            if (!host) return nullptr;
            void* track = static_cast<HirariUnifiedEngine*>(host)->findPublishedTrack(trackId);
            return track ? hirari_track_storage(track, kHirariTrackStoreArticulation) : nullptr;
        },
        [](void* host, uint32_t trackId, uint8_t kind, const uint8_t* data,
           size_t length) -> bool {
            if (!host || (length != 0 && !data)) return false;
            void* track = static_cast<HirariUnifiedEngine*>(host)->findPublishedTrack(trackId);
            return hirari_track_expression_map_set(track, kind, data, length);
        },
    };
    hirari_expression_map_edit_host_install(&ops);
}
