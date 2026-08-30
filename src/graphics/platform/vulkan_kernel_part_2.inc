#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
bool VulkanGraphicsKernel::initializeWithSurface(VkInstance instance, VkSurfaceKHR surface,
                                                 uint32_t width, uint32_t height) {
    cleanup();
    auto& context = Aura::Rendering::Vulkan::VulkanContext::getInstance();
    if (!context.initialize(instance, surface)) {
        m_lastError = context.lastError();
        return false;
    }
    if (!context.createSwapchain(width, height) || !context.areUiShaderModulesReady()) {
        m_lastError = context.lastError();
        context.shutdown();
        return false;
    }
    m_contextReady = true;
    m_runtimeReady = true;
    m_available = true;
    m_lastError.clear();
    return true;
}
#endif

void VulkanGraphicsKernel::beginFrame() {
#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
    if (m_contextReady) {
        Aura::Rendering::Vulkan::VulkanContext::getInstance().beginUiFrame();
    }
#endif
    m_drawCallCount.store(0, std::memory_order_relaxed);
    m_vertexCount.store(0, std::memory_order_relaxed);
    m_scissorDepth = 0;
}

void VulkanGraphicsKernel::endFrame() {
#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
    if (m_contextReady && !Aura::Rendering::Vulkan::VulkanContext::getInstance().drawEmptyFrame()) {
        m_lastError = Aura::Rendering::Vulkan::VulkanContext::getInstance().lastError();
    }
#endif
    const auto now = std::chrono::steady_clock::now();
    const uint64_t micros = static_cast<uint64_t>(
        std::chrono::duration_cast<std::chrono::microseconds>(now.time_since_epoch()).count());
    const uint64_t previous = m_lastFrameMicros.exchange(micros, std::memory_order_relaxed);
    if (previous != 0 && micros > previous) {
        const float instantFps = 1'000'000.0f / static_cast<float>(micros - previous);
        const float old = m_fps.load(std::memory_order_relaxed);
        m_fps.store(old <= 0.0f ? instantFps : old * 0.9f + instantFps * 0.1f,
                    std::memory_order_relaxed);
    }
}

void VulkanGraphicsKernel::updateSpectrogram(const std::vector<float>& data) {
    constexpr size_t kMaxBins = 4096;
    std::lock_guard<std::mutex> lock(m_spectrogramMutex);
    m_spectrogramData.clear();
    m_spectrogramData.reserve(std::min(data.size(), kMaxBins));
    for (size_t i = 0; i < data.size() && i < kMaxBins; ++i) {
        const float value = data[i];
        m_spectrogramData.push_back(std::isfinite(value) ? std::clamp(value, 0.0f, 1.0f) : 0.0f);
    }
    m_spectrogramRevision.fetch_add(1, std::memory_order_release);
}

void VulkanGraphicsKernel::drawMeter(uint32_t trackId, float levelL, float levelR,
                                    float x, float y, float w, float h) {
    (void)trackId;
    if (!m_available || !m_contextReady || w <= 0.0f || h <= 0.0f) return;
    const auto drawChannel = [this, x, y, w, h](float level, float offset, uint32_t color) {
        const float normalized = std::clamp(std::isfinite(level) ? level : 0.0f, 0.0f, 1.0f);
        constexpr uint32_t segments = 24;
        const float gap = std::max(1.0f, h / 160.0f);
        const float segmentHeight = std::max(1.0f, (h - gap * (segments - 1)) / segments);
        const uint32_t lit = static_cast<uint32_t>(std::round(normalized * segments));
        for (uint32_t index = 0; index < segments; ++index) {
            const float segmentY = y + h - (index + 1) * segmentHeight - index * gap;
            const uint32_t segmentColor = index < lit ? color : 0x301f2528u;
            SDFPushConstants pc{x + offset, segmentY, w, segmentHeight,
                                segmentColor, segmentColor, segmentColor, segmentColor,
                                1.0f, 0.0f, 0.0f, 0.0f};
            pushSDF(pc);
        }
    };
    const float channelWidth = std::max(1.0f, (w - 2.0f) * 0.48f);
    drawChannel(levelL, 0.0f, levelL > 0.92f ? 0xff4040e8u : (levelL > 0.72f ? 0xff40b8d8u : 0xff48a86au));
    drawChannel(levelR, channelWidth + 2.0f, levelR > 0.92f ? 0xff4040e8u : (levelR > 0.72f ? 0xff40b8d8u : 0xff48a86au));
}

void VulkanGraphicsKernel::drawWaveformPath(const float* minData, const float* maxData,
                                            size_t count, float x, float centerY,
                                            float width, float heightScale, uint32_t color) {
    if (!m_available || !m_contextReady || !minData || !maxData || count == 0 || width <= 0.0f) return;
    const size_t visible = std::min<size_t>(count, 1024);
    const float columnWidth = std::max(1.0f, width / static_cast<float>(visible));
    for (size_t index = 0; index < visible; ++index) {
        const float lo = std::isfinite(minData[index]) ? minData[index] : 0.0f;
        const float hi = std::isfinite(maxData[index]) ? maxData[index] : 0.0f;
        const float top = centerY - std::max(lo, hi) * heightScale;
        const float bottom = centerY - std::min(lo, hi) * heightScale;
        const float drawHeight = std::max(1.0f, bottom - top);
        SDFPushConstants pc{x + index * columnWidth, top, columnWidth, drawHeight,
                            color, color, color, color,
                            0.0f, 0.0f, 0.0f, 0.0f};
        pushSDF(pc);
    }
}

void VulkanGraphicsKernel::drawBrushedCircle(float x, float y, float radius, uint32_t color) {
    drawCircle(x, y, radius, color);
}

void VulkanGraphicsKernel::drawNative(float x, float y, float w, float h, float p1, float p2,
                                      float p3, uint32_t c1, uint32_t c2, float type) {
    (void)p1; (void)p2; (void)p3; (void)c2; (void)type;
    drawRoundedRect(x, y, w, h, std::max(0.0f, std::min(w, h) * 0.08f), c1);
}

void VulkanGraphicsKernel::drawIconAudio(float x, float y, float s, uint32_t col) {
    for (int i = 0; i < 4; ++i) {
        const float height = s * (0.25f + 0.18f * static_cast<float>((i + 1) % 3));
        drawRoundedRect(x + s * (0.12f + i * 0.22f), y + (s - height) * 0.5f,
                        std::max(1.0f, s * 0.12f), height, s * 0.04f, col);
    }
}

void VulkanGraphicsKernel::drawIconInstrument(float x, float y, float s, uint32_t col) {
    drawRoundedRect(x, y + s * 0.22f, s, s * 0.56f, s * 0.08f, col);
    for (int i = 1; i < 7; ++i)
        drawRoundedRect(x + s * i / 7.0f, y + s * 0.24f, std::max(1.0f, s * 0.025f), s * 0.52f, 0.0f, 0xff101216u);
}

void VulkanGraphicsKernel::drawIconMidi(float x, float y, float s, uint32_t col) {
    drawRoundedRect(x + s * 0.08f, y + s * 0.55f, s * 0.84f, s * 0.18f, s * 0.04f, col);
    for (int i = 0; i < 3; ++i)
        drawRoundedRect(x + s * (0.16f + i * 0.26f), y + s * 0.2f,
                        s * 0.14f, s * 0.35f, s * 0.04f, col);
}

void VulkanGraphicsKernel::drawIconMic(float x, float y, float s, uint32_t col) {
    drawRoundedRect(x + s * 0.3f, y + s * 0.08f, s * 0.4f, s * 0.62f, s * 0.2f, col);
    drawRoundedRect(x + s * 0.18f, y + s * 0.46f, s * 0.64f, s * 0.14f, s * 0.06f, col);
    drawRoundedRect(x + s * 0.46f, y + s * 0.6f, std::max(1.0f, s * 0.08f), s * 0.25f, 0.0f, col);
}

void VulkanGraphicsKernel::drawIconDrums(float x, float y, float s, uint32_t col) {
    drawCircle(x + s * 0.32f, y + s * 0.58f, s * 0.22f, col);
    drawCircle(x + s * 0.68f, y + s * 0.58f, s * 0.22f, col);
    drawRoundedRect(x + s * 0.14f, y + s * 0.12f, s * 0.72f, s * 0.12f, s * 0.04f, col);
}

void VulkanGraphicsKernel::drawTriangle(float x1, float y1, float x2, float y2,
                                        float x3, float y3, float thickness, uint32_t color) {
    (void)thickness;
    drawFilledTriangle(x1, y1, x2, y2, x3, y3, color);
}

void VulkanGraphicsKernel::pushSDF(const SDFPushConstants& push) {
#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
    if (!m_available || !m_contextReady) return;
    Aura::Rendering::Vulkan::VulkanContext::getInstance().setUiRectPixels(
        push.x, push.y, push.w, push.h, static_cast<uint32_t>(push.colorL));
    m_drawCallCount.fetch_add(1, std::memory_order_relaxed);
    m_vertexCount.fetch_add(4, std::memory_order_relaxed);
#else
    (void)push;
#endif
}

void VulkanGraphicsKernel::calculateBezier(float x1, float y1, float cp1x, float cp1y,
                                           float cp2x, float cp2y, float x2, float y2,
                                           float t, float& ox, float& oy) const {
    const float u = 1.0f - std::clamp(t, 0.0f, 1.0f);
    const float tt = 1.0f - u;
    ox = u * u * u * x1 + 3.0f * u * u * tt * cp1x + 3.0f * u * tt * tt * cp2x + tt * tt * tt * x2;
    oy = u * u * u * y1 + 3.0f * u * u * tt * cp1y + 3.0f * u * tt * tt * cp2y + tt * tt * tt * y2;
}

void VulkanGraphicsKernel::drawBezierCurve(float x1, float y1, float cp1x, float cp1y,
                                           float cp2x, float cp2y, float x2, float y2,
                                           float thickness, uint32_t color) {
    if (!m_available || !m_contextReady || !std::isfinite(thickness)) return;
    constexpr int kSegments = 32;
    float previousX = x1;
    float previousY = y1;
    for (int index = 1; index <= kSegments; ++index) {
        float currentX = 0.0f;
        float currentY = 0.0f;
        calculateBezier(x1, y1, cp1x, cp1y, cp2x, cp2y, x2, y2,
                        static_cast<float>(index) / kSegments, currentX, currentY);
        drawLine(previousX, previousY, currentX, currentY, thickness, color);
        previousX = currentX;
        previousY = currentY;
    }
}

IGraphicsKernel::RenderingTelemetry VulkanGraphicsKernel::getTelemetry() const {
    return {m_fps.load(std::memory_order_relaxed),
            m_drawCallCount.load(std::memory_order_relaxed),
            m_vertexCount.load(std::memory_order_relaxed), 0.0f};
}

void VulkanGraphicsKernel::pushScissor(float x, float y, float w, float h) {
    if (m_scissorDepth >= m_scissors.size()) return;
    Scissor next{x, y, std::max(0.0f, w), std::max(0.0f, h)};
    if (m_scissorDepth > 0) {
        const Scissor& parent = m_scissors[m_scissorDepth - 1];
        const float right = std::min(parent.x + parent.w, next.x + next.w);
        const float bottom = std::min(parent.y + parent.h, next.y + next.h);
        next.x = std::max(parent.x, next.x);
        next.y = std::max(parent.y, next.y);
        next.w = std::max(0.0f, right - next.x);
        next.h = std::max(0.0f, bottom - next.y);
    }
    m_scissors[m_scissorDepth++] = next;
#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
    if (m_contextReady) {
        Aura::Rendering::Vulkan::VulkanContext::getInstance().setUiScissor(
            next.x, next.y, next.w, next.h);
    }
#endif
}

void VulkanGraphicsKernel::popScissor() {
    if (m_scissorDepth > 0) --m_scissorDepth;
#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
    if (m_contextReady) {
        auto& context = Aura::Rendering::Vulkan::VulkanContext::getInstance();
        if (m_scissorDepth > 0) {
            const Scissor& current = m_scissors[m_scissorDepth - 1];
            context.setUiScissor(current.x, current.y, current.w, current.h);
        } else {
            context.setUiScissor(0.0f, 0.0f, 1.0e9f, 1.0e9f);
        }
    }
#endif
}

void VulkanGraphicsKernel::drawRoundedRect(float x, float y, float w, float h, float radius, uint32_t color) {
    SDFPushConstants pc { x, y, w, h, color, color, color, color, radius, 0.0f, 0.0f, 0.0f };
    pushSDF(pc);
}

void VulkanGraphicsKernel::drawGradientRect(float x, float y, float w, float h, uint32_t colorTop, uint32_t colorBottom) {
    SDFPushConstants pc { x, y, w, h, colorTop, colorTop, colorBottom, colorBottom, 0.0f, 0.0f, 0.0f, 0.0f };
    pushSDF(pc);
}

void VulkanGraphicsKernel::drawText(const std::string& text, float x, float y, float size, uint32_t color) {
    // --- VULKAN BATCHING: Signed Distance Field (SDF) Font Atlas ---
    // Professional DAWs use SDF for pin-sharp text at any zoom level.
    // Each character is a quad indexing into the Global Font Descriptor Set.
    // MSDF (Multi-channel SDF) rendering logic
    for (char c : text) {
        float charW = size * 0.6f;
        SDFPushConstants pc { x, y, charW, size, color, color, color, color,
                              0.0f, 2.0f, static_cast<float>(static_cast<unsigned char>(c)), 0.0f };
        pushSDF(pc);
        x += charW;
    }
}
float VulkanGraphicsKernel::measureText(const std::string& text, float size) const { 
    // Proportional Font Metrics (Logic Pro Logic)
    return text.length() * size * 0.58f; 
}

void VulkanGraphicsKernel::drawCircle(float x, float y, float radius, uint32_t color) {
    drawRoundedRect(x - radius, y - radius, radius * 2, radius * 2, radius, color);
}

void VulkanGraphicsKernel::drawLine(float x1, float y1, float x2, float y2, float thickness, uint32_t color) {
    if (!m_available || !m_contextReady || !std::isfinite(x1) || !std::isfinite(y1) ||
        !std::isfinite(x2) || !std::isfinite(y2)) return;
    const float dx = x2 - x1, dy = y2 - y1;
    const float length = std::sqrt(dx * dx + dy * dy);
    if (length <= 0.01f) { drawCircle(x1, y1, std::max(0.5f, thickness * 0.5f), color); return; }
    // The shared UI shader currently accepts axis-aligned SDF quads.  Stamp
    // small circles along the segment so diagonal lines remain visible until
    // the dedicated rotated-quad pipeline is enabled.
    const uint32_t steps = std::min<uint32_t>(256, std::max<uint32_t>(1, static_cast<uint32_t>(std::ceil(length / 2.0f))));
    const float radius = std::max(0.5f, thickness * 0.5f);
    for (uint32_t i = 0; i <= steps; ++i) {
        const float t = static_cast<float>(i) / steps;
        drawCircle(x1 + dx * t, y1 + dy * t, radius, color);
    }
}

void VulkanGraphicsKernel::drawArc(float cx, float cy, float r, float start, float end, float thick, uint32_t color) {
    if (!m_available || !m_contextReady || r <= 0.0f) return;
    constexpr float pi = 3.14159265358979323846f;
    const float span = end - start;
    const uint32_t steps = std::min<uint32_t>(256, std::max<uint32_t>(2, static_cast<uint32_t>(std::ceil(std::fabs(span) / 8.0f))));
    for (uint32_t i = 0; i <= steps; ++i) {
        const float a = (start + span * static_cast<float>(i) / steps) * pi / 180.0f;
        drawCircle(cx + std::cos(a) * r, cy + std::sin(a) * r, std::max(0.5f, thick * 0.5f), color);
    }
}

void VulkanGraphicsKernel::drawVertexPath(const Vertex* vertices, size_t count, float thickness) {
    if (!vertices || count == 0) return;
    for (size_t i = 1; i < count; ++i)
        drawLine(vertices[i - 1].x, vertices[i - 1].y, vertices[i].x, vertices[i].y,
                 thickness, vertices[i].color);
}

void VulkanGraphicsKernel::drawVertexPathFilled(const Vertex* vertices, size_t count) {
    if (!vertices || count == 0) return;
    const Vertex& origin = vertices[0];
    for (size_t i = 2; i < count; ++i)
        drawFilledTriangle(origin.x, origin.y, vertices[i - 1].x, vertices[i - 1].y,
                           vertices[i].x, vertices[i].y, vertices[i].color);
}

void VulkanGraphicsKernel::drawFilledPath(const std::vector<float>& points, uint32_t color) {
    if (points.size() < 6 || (points.size() & 1u)) return;
    const float ox = points[0], oy = points[1];
    for (size_t i = 4; i + 1 < points.size(); i += 2)
        drawFilledTriangle(ox, oy, points[i - 2], points[i - 1], points[i], points[i + 1], color);
}

void VulkanGraphicsKernel::drawDropShadow(float x, float y, float w, float h, float radius, uint32_t color) {
    if (!m_available || !m_contextReady || w <= 0.0f || h <= 0.0f) return;
    const uint32_t passes = std::min<uint32_t>(8, std::max<uint32_t>(1, static_cast<uint32_t>(radius)));
    for (uint32_t i = passes; i > 0; --i) {
        const float t = static_cast<float>(i) / passes;
        drawRoundedRect(x - i * 0.5f, y - i * 0.5f, w + i, h + i, radius + i,
                        (color & 0x00ffffffu) | (static_cast<uint32_t>(((color >> 24) & 0xffu) * t * 0.18f) << 24));
    }
}

void VulkanGraphicsKernel::applyBlurEffect(float x, float y, float w, float h, float intensity) {
    const uint32_t passes = std::min<uint32_t>(6, std::max<uint32_t>(1, static_cast<uint32_t>(intensity * 4.0f)));
    for (uint32_t i = 1; i <= passes; ++i)
        drawRoundedRect(x - i, y - i, w + 2.0f * i, h + 2.0f * i, static_cast<float>(i), 0x12000000u);
}

void VulkanGraphicsKernel::drawBezierPath(const std::vector<float>& points, uint32_t color, float thickness) {
    if (points.size() < 4 || (points.size() & 1u)) return;
    for (size_t i = 3; i < points.size(); i += 2)
        drawLine(points[i - 3], points[i - 2], points[i - 1], points[i], thickness, color);
}


void VulkanGraphicsKernel::drawFilledTriangle(float x1, float y1, float x2, float y2,
                                              float x3, float y3, uint32_t color) {
    if (!m_available || !m_contextReady) return;
    const float minY = std::min({y1, y2, y3});
    const float maxY = std::max({y1, y2, y3});
    const uint32_t rows = std::min<uint32_t>(128, std::max<uint32_t>(1, static_cast<uint32_t>(std::ceil(maxY - minY))));
    const auto intersect = [](float ay, float ax, float by, float bx, float y, float& out) {
        if ((y < std::min(ay, by)) || (y > std::max(ay, by)) || ay == by) return false;
        out = ax + (y - ay) * (bx - ax) / (by - ay);
        return std::isfinite(out);
    };
    for (uint32_t row = 0; row < rows; ++row) {
        const float y = minY + (maxY - minY) * (static_cast<float>(row) + 0.5f) / rows;
        float points[3]{};
        uint32_t count = 0;
        count += intersect(y1, x1, y2, x2, y, points[count]);
        count += intersect(y2, x2, y3, x3, y, points[count]);
        count += intersect(y3, x3, y1, x1, y, points[count]);
        if (count < 2) continue;
        const float left = std::min(points[0], points[1]);
        const float right = std::max(points[0], points[1]);
        SDFPushConstants pc{left, y, std::max(1.0f, right - left),
                            std::max(1.0f, (maxY - minY) / rows),
                            color, color, color, color,
                            0.0f, 0.0f, 0.0f, 0.0f};
        pushSDF(pc);
    }
}
void VulkanGraphicsKernel::drawNeonRect(float x, float y, float w, float h, float radius, float glow, uint32_t color) {
    SDFPushConstants pc { x, y, w, h, color, color, color, color, radius, 0.0f, 0.0f, glow };
    pushSDF(pc);
}

void VulkanGraphicsKernel::drawGoniometer(float x, float y, float w, float h, const float* historyL, const float* historyR, size_t count) {
    if (!historyL || !historyR || count == 0 || !m_available || !m_contextReady) return;
    const float cx = x + w * 0.5f, cy = y + h * 0.5f;
    drawLine(x, cy, x + w, cy, 1.0f, 0x332f3a44u);
    drawLine(cx, y, cx, y + h, 1.0f, 0x332f3a44u);
    const size_t visible = std::min<size_t>(count, 512);
    for (size_t i = 1; i < visible; ++i) {
        const size_t a = count - visible + i - 1, b = count - visible + i;
        if (a >= count || b >= count) continue;
        drawLine(cx + historyL[a] * w * 0.45f, cy - historyR[a] * h * 0.45f,
                 cx + historyL[b] * w * 0.45f, cy - historyR[b] * h * 0.45f, 1.0f, 0xff48a8a0u);
    }
}

void VulkanGraphicsKernel::cleanup() {
#if defined(AURA_ENABLE_VULKAN) && AURA_ENABLE_VULKAN
    if (m_contextReady) {
        Aura::Rendering::Vulkan::VulkanContext::getInstance().shutdown();
    }
    if (m_surface != VK_NULL_HANDLE && m_instance != VK_NULL_HANDLE) {
        vkDestroySurfaceKHR(m_instance, m_surface, nullptr);
    }
    if (m_device != VK_NULL_HANDLE) {
        vkDeviceWaitIdle(m_device);
        vkDestroyDevice(m_device, nullptr);
    }
    if (m_instance != VK_NULL_HANDLE) {
        vkDestroyInstance(m_instance, nullptr);
    }
    m_instance = VK_NULL_HANDLE;
    m_physicalDevice = VK_NULL_HANDLE;
    m_device = VK_NULL_HANDLE;
    m_graphicsQueue = VK_NULL_HANDLE;
    m_surface = VK_NULL_HANDLE;
    m_pipelineLayout = VK_NULL_HANDLE;
#else
    m_instance = nullptr;
    m_physicalDevice = nullptr;
    m_device = nullptr;
    m_graphicsQueue = nullptr;
    m_surface = nullptr;
    m_pipelineLayout = nullptr;
#endif
    m_available = false;
    m_runtimeReady = false;
    m_contextReady = false;
    m_graphicsQueueFamily = UINT32_MAX;
}
