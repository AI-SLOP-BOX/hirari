#pragma once
#include <string>
#include <functional>
#include <vector>
#include <memory>
#include <chrono>
#include <mutex>
#include <algorithm>
#include <array>
#include "../../graphics/graphics_kernel.hpp"
#include "../../ui/main/workspace.hpp"
#include "../../core/log_buffer.hpp"
#include "../../core/concurrency/lock_free.hpp"

namespace Aura::UI::Main {

/**
 * @class MainViewOrchestrator
 * @brief High-level UI lifecycle and frame orchestration.
 * HONEST FIX: Replaced 'Bootstrap' fluff with real performance monitoring and clean shutdown.
 */
class MainViewOrchestrator {
public:
    enum class InputType : uint8_t { MouseDown, MouseDrag, MouseUp, KeyDown };
    struct InputCommand {
        InputType type = InputType::MouseDown;
        float x = 0.0f;
        float y = 0.0f;
        uint16_t key = 0;
        bool command = false;
        bool shift = false;
    };
    static MainViewOrchestrator& getInstance() {
        static MainViewOrchestrator instance;
        return instance;
    }

    explicit MainViewOrchestrator(WorkspaceManager& workspace)
        : m_workspace(&workspace) {}

    void initialize(void* windowHandle, float w, float h) {
        std::lock_guard<std::mutex> lock(m_mutex);
        ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "UI_INITIALIZING");
        m_running = true;
        m_width = w;
        m_height = h;
        
        m_kernel = Graphics::Platform::GraphicsFactory::createDefault();
        if (m_kernel) {
            m_kernel->initialize(windowHandle);
        }
        
        m_workspace->initialize(w, h);
    }

    void shutdown() {
        std::lock_guard<std::mutex> lock(m_mutex);
        ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "UI_SHUTTING_DOWN");
        m_running = false;
        m_kernel.reset();
    }

    void renderFrame() {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (!m_kernel) return;

        auto start = std::chrono::steady_clock::now();
        drainInputCommands();

        m_kernel->beginFrame();
        m_workspace->render(*m_kernel);
        m_kernel->endFrame();

        auto end = std::chrono::steady_clock::now();
        m_lastFrameDurationMs = std::chrono::duration<float, std::milli>(end - start).count();
    }

    // Compatibility entry points used by the native window host.
    void bootstrap(void* windowHandle, float w, float h) { initialize(windowHandle, w, h); }
    void updateUI() { renderFrame(); }

    void onResize(float w, float h) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_width = std::max(0.0f, w);
        m_height = std::max(0.0f, h);
        m_workspace->initialize(m_width, m_height);
    }

    void setScale(float scale) {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_kernel) m_kernel->setScale(std::max(0.1f, scale));
    }

    // Input is serialized with frame rendering, then routed to the workspace.
    void handleMouseDown(float x, float y) {
        enqueue({InputType::MouseDown, x, y, 0, false, false});
    }
    void handleMouseDrag(float x, float y) {
        enqueue({InputType::MouseDrag, x, y, 0, false, false});
    }
    void handleMouseUp(float x, float y) {
        enqueue({InputType::MouseUp, x, y, 0, false, false});
    }
    void handleKeyDown(unsigned short key, bool command, bool shift) {
        enqueue({InputType::KeyDown, 0.0f, 0.0f, key, command, shift});
    }

    float getLastFrameDurationMs() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_lastFrameDurationMs;
    }
    bool isRunning() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_running;
    }

    ::Aura::Graphics::Platform::IGraphicsKernel* tryGetKernel() noexcept {
        return m_kernel.get();
    }

private:
    MainViewOrchestrator() : m_workspace(&AuraWorkspace::getInstance()) {}

    void enqueue(const InputCommand& command) noexcept {
        if (!m_inputQueue.push(command)) {
            m_droppedInputCommands.fetch_add(1, std::memory_order_relaxed);
        }
    }

    void drainInputCommands() {
        InputCommand command;
        while (m_inputQueue.pop(command)) {
            auto& workspace = *m_workspace;
            switch (command.type) {
                case InputType::MouseDown: workspace.handleMouseDown(command.x, command.y); break;
                case InputType::MouseDrag: workspace.handleMouseDrag(command.x, command.y); break;
                case InputType::MouseUp: workspace.handleMouseUp(command.x, command.y); break;
                case InputType::KeyDown:
                    // The density switch is intentionally a command shortcut,
                    // so an accidental number key during recording cannot
                    // rearrange the workspace.  It also gives keyboard-first
                    // and Computer Use clients a stable UI contract.
                    if (command.command && command.key == 49) {
                        workspace.setExperienceMode(ExperienceMode::Beginner);
                    } else if (command.command && command.key == 50) {
                        workspace.setExperienceMode(ExperienceMode::Pro);
                    } else if (command.command && command.key == 51) {
                        workspace.setVisiblePanels(workspace.visiblePanels());
                    } else {
                        workspace.handleKeyDown(static_cast<int>(command.key));
                    }
                    break;
            }
        }
    }

    bool m_running = false;
    WorkspaceManager* m_workspace = nullptr;
    float m_width = 1280, m_height = 800;
    float m_lastFrameDurationMs = 0.0f;
    std::unique_ptr<::Aura::Graphics::Platform::IGraphicsKernel> m_kernel;
    mutable std::mutex m_mutex;
    ::Aura::Core::Concurrency::MPMCQueue<InputCommand, 256> m_inputQueue;
    std::atomic<uint64_t> m_droppedInputCommands{0};
};

using AuraAppView = MainViewOrchestrator;

} // namespace Aura::UI::Main
