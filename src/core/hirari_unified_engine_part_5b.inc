bool HirariUnifiedEngine::bounce_project_async(std::string_view path, uint32_t format) {
    if (format > 3 || path.empty()) {
        m_bounceProgress.store(0.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
        return false;
    }
    std::lock_guard<std::mutex> bounceLifecycleLock(m_bounceLifecycleMutex);
    if (m_bounceRunning.load(std::memory_order_acquire)) {
        const uint32_t state = m_bounceState.load(std::memory_order_acquire);
        if (!hirari_bounce_state_terminal(state)) return false;
        // A terminal state is published just before the worker wrapper clears
        // m_bounceRunning. Join here so a retry cannot be rejected during
        // that narrow retirement window.
        if (m_bounceThread.joinable()) m_bounceThread.join();
        m_bounceRunning.store(false, std::memory_order_release);
    } else if (m_bounceThread.joinable()) {
        m_bounceThread.join();
    }
    m_bounceRunning.store(true, std::memory_order_release);
    m_bounceCancelRequested.store(false, std::memory_order_release);
    m_bouncePauseRequested.store(false, std::memory_order_release);
    m_bounceProgress.store(0.0f, std::memory_order_release);
    m_bounceState.store(static_cast<uint32_t>(BounceState::Queued), std::memory_order_release);
    const std::string outputPath(path);
    m_bounceThread = std::thread([this, outputPath, format]() {
        const bool success = bounce_project(outputPath, format);
        const bool cancelled = get_bounce_state() == static_cast<uint32_t>(BounceState::Cancelled);
        ::Hirari::Core::Diagnostics::LogBuffer::post(
            success ? 2 : (cancelled ? 1 : 0), 0,
            success ? "BOUNCE_COMPLETE" : (cancelled ? "BOUNCE_CANCELLED" : "BOUNCE_FAILED"));
        m_bounceRunning.store(false, std::memory_order_release);
    });
    return true;
}

float HirariUnifiedEngine::get_bounce_progress() const noexcept {
    return std::clamp(m_bounceProgress.load(std::memory_order_acquire), 0.0f, 1.0f);
}

uint32_t HirariUnifiedEngine::get_bounce_state() const noexcept {
    return m_bounceState.load(std::memory_order_acquire);
}

bool HirariUnifiedEngine::cancel_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (!hirari_bounce_state_cancellable(state)) return false;
    {
        std::lock_guard<std::mutex> publishLock(m_bouncePublishMutex);
        m_bounceCancelRequested.store(true, std::memory_order_release);
        m_bouncePauseRequested.store(false, std::memory_order_release);
    }
    return true;
}

bool HirariUnifiedEngine::pause_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (!hirari_bounce_state_pausable(state)) return false;
    m_bouncePauseRequested.store(true, std::memory_order_release);
    return true;
}

bool HirariUnifiedEngine::resume_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (!hirari_bounce_state_resumable(state)) return false;
    m_bouncePauseRequested.store(false, std::memory_order_release);
    return true;
}

bool HirariUnifiedEngine::set_project_scale(int32_t root, int32_t type) {
    // The bridge validates the ranges before this is reached.
    m_rootNote.store(root, std::memory_order_relaxed);
    m_scaleType.store(type, std::memory_order_relaxed);
    return true;
}

bool HirariUnifiedEngine::set_articulation_map(uint32_t trackId, uint32_t) {
    return findPublishedTrack(trackId) != nullptr;
}



void HirariUnifiedEngine::installExpressionMapEditHost() {
    const HirariExpressionMapEditHostOps ops{
        [](void* host, uint32_t trackId) -> void* {
            if (!host) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            Track* track = engine->findPublishedTrack(trackId);
            return track ? track->articulationStateHandle() : nullptr;
        },
        [](void* host, uint32_t trackId, uint8_t kind, const uint8_t* data,
           size_t length) -> bool {
            if (!host || (length != 0 && !data)) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            Track* track = engine->findPublishedTrack(trackId);
            if (!track) return false;
            std::string json(reinterpret_cast<const char*>(data), length);
            if (kind == 0) track->setExpressionMapJson(std::move(json));
            else track->setExpressionMapProJson(std::move(json));
            return true;
        },
    };
    hirari_expression_map_edit_host_install(&ops);
}
