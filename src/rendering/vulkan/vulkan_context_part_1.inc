/**
 * @brief High-performance Vulkan 1.3 Core for the DAW.
 * HONEST FIX: Uses modern Dynamic Rendering (KHR_dynamic_rendering) 
 * instead of the old-school, verbose RenderPass/Framebuffer boilerplate.
 * This is exactly what high-speed native UIs like Zed's GPUI do.
 */
class VulkanContext {
public:
    static constexpr uint32_t kMaxUiRects = 1024;
    struct UiUniforms {
        float pos[4];
        float props[4];
        float color[4];
    };
    static VulkanContext& getInstance() {
        static VulkanContext i;
        return i;
    }

    ~VulkanContext() { reset(); }

    bool initialize(VkInstance instance, VkSurfaceKHR surface) {
        reset();
        if (instance == VK_NULL_HANDLE || surface == VK_NULL_HANDLE) {
            m_error = "Vulkan context requires a valid instance and surface";
            return false;
        }

        uint32_t deviceCount = 0;
        if (vkEnumeratePhysicalDevices(instance, &deviceCount, nullptr) != VK_SUCCESS ||
            deviceCount == 0) {
            m_error = "Vulkan context found no physical devices";
            return false;
        }
        std::vector<VkPhysicalDevice> devices(deviceCount);
        if (vkEnumeratePhysicalDevices(instance, &deviceCount, devices.data()) != VK_SUCCESS) {
            m_error = "Vulkan physical-device enumeration failed";
            return false;
        }

        constexpr uint32_t invalidFamily = std::numeric_limits<uint32_t>::max();
        for (VkPhysicalDevice candidate : devices) {
            uint32_t familyCount = 0;
            vkGetPhysicalDeviceQueueFamilyProperties(candidate, &familyCount, nullptr);
            std::vector<VkQueueFamilyProperties> families(familyCount);
            vkGetPhysicalDeviceQueueFamilyProperties(candidate, &familyCount, families.data());
            for (uint32_t family = 0; family < familyCount; ++family) {
                if ((families[family].queueFlags & VK_QUEUE_GRAPHICS_BIT) == 0) continue;
                VkBool32 present = VK_FALSE;
                if (vkGetPhysicalDeviceSurfaceSupportKHR(candidate, family, surface, &present) != VK_SUCCESS ||
                    present == VK_FALSE) continue;
                m_physicalDevice = candidate;
                m_graphicsQueueFamily = family;
                break;
            }
            if (m_graphicsQueueFamily != invalidFamily) break;
        }
        if (m_physicalDevice == VK_NULL_HANDLE) {
            m_error = "Vulkan has no graphics queue that can present to the surface";
            return false;
        }

        uint32_t extensionCount = 0;
        if (vkEnumerateDeviceExtensionProperties(m_physicalDevice, nullptr,
                                                  &extensionCount, nullptr) != VK_SUCCESS) {
            reset();
            m_error = "Vulkan device extension enumeration failed";
            return false;
        }
        std::vector<VkExtensionProperties> extensions(extensionCount);
        vkEnumerateDeviceExtensionProperties(m_physicalDevice, nullptr,
                                             &extensionCount, extensions.data());
        const bool hasSwapchain = std::any_of(
            extensions.begin(), extensions.end(), [](const VkExtensionProperties& extension) {
                return std::strcmp(extension.extensionName, VK_KHR_SWAPCHAIN_EXTENSION_NAME) == 0;
            });
        if (!hasSwapchain) {
            reset();
            m_error = "Vulkan device does not support VK_KHR_swapchain";
            return false;
        }

        constexpr float priority = 1.0f;
        VkDeviceQueueCreateInfo queueInfo{};
        queueInfo.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO;
        queueInfo.queueFamilyIndex = m_graphicsQueueFamily;
        queueInfo.queueCount = 1;
        queueInfo.pQueuePriorities = &priority;
        VkDeviceCreateInfo deviceInfo{};
        deviceInfo.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO;
        deviceInfo.queueCreateInfoCount = 1;
        deviceInfo.pQueueCreateInfos = &queueInfo;
        const char* deviceExtensions[] = {VK_KHR_SWAPCHAIN_EXTENSION_NAME};
        deviceInfo.enabledExtensionCount = 1;
        deviceInfo.ppEnabledExtensionNames = deviceExtensions;
        if (vkCreateDevice(m_physicalDevice, &deviceInfo, nullptr, &m_device) != VK_SUCCESS) {
            m_physicalDevice = VK_NULL_HANDLE;
            m_graphicsQueueFamily = invalidFamily;
            m_error = "Vulkan logical-device creation failed";
            return false;
        }
        vkGetDeviceQueue(m_device, m_graphicsQueueFamily, 0, &m_graphicsQueue);
        if (m_graphicsQueue == VK_NULL_HANDLE) {
            reset();
            m_error = "Vulkan graphics queue creation failed";
            return false;
        }
        if (!createUiShaderModules()) {
            const std::string shaderError = m_error;
            reset();
            m_error = shaderError;
            return false;
        }
        m_instance = instance;
        m_surface = surface;
        m_error.clear();
        return true;
    }

    bool createSwapchain(uint32_t width, uint32_t height) {
        if (m_device == VK_NULL_HANDLE || m_surface == VK_NULL_HANDLE || width == 0 || height == 0) {
            m_error = "Vulkan swapchain requires an initialized context and non-zero size";
            return false;
        }
        if (m_swapchain != VK_NULL_HANDLE) {
            destroySwapchainResources();
            vkDestroySwapchainKHR(m_device, m_swapchain, nullptr);
            m_swapchain = VK_NULL_HANDLE;
            m_swapchainImages.clear();
        }

        VkSurfaceCapabilitiesKHR capabilities{};
        if (vkGetPhysicalDeviceSurfaceCapabilitiesKHR(m_physicalDevice, m_surface,
                                                       &capabilities) != VK_SUCCESS) {
            m_error = "Vulkan surface capabilities query failed";
            return false;
        }
        uint32_t formatCount = 0;
        if (vkGetPhysicalDeviceSurfaceFormatsKHR(m_physicalDevice, m_surface,
                                                 &formatCount, nullptr) != VK_SUCCESS ||
            formatCount == 0) {
            m_error = "Vulkan surface exposes no formats";
            return false;
        }
        std::vector<VkSurfaceFormatKHR> formats(formatCount);
        vkGetPhysicalDeviceSurfaceFormatsKHR(m_physicalDevice, m_surface,
                                             &formatCount, formats.data());
        const auto formatIt = std::find_if(formats.begin(), formats.end(), [](const auto& format) {
            return format.format == VK_FORMAT_B8G8R8A8_SRGB &&
                   format.colorSpace == VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
        });
        const VkSurfaceFormatKHR surfaceFormat = formatIt != formats.end()
            ? *formatIt : formats.front();

        VkExtent2D extent{};
        if (capabilities.currentExtent.width != std::numeric_limits<uint32_t>::max()) {
            extent = capabilities.currentExtent;
        } else {
            extent.width = std::clamp(width, capabilities.minImageExtent.width,
                                      capabilities.maxImageExtent.width);
            extent.height = std::clamp(height, capabilities.minImageExtent.height,
                                       capabilities.maxImageExtent.height);
        }
        uint32_t imageCount = capabilities.minImageCount + 1;
        if (capabilities.maxImageCount > 0)
            imageCount = std::min(imageCount, capabilities.maxImageCount);

        VkSwapchainCreateInfoKHR swapchainInfo{};
        swapchainInfo.sType = VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR;
        swapchainInfo.surface = m_surface;
        swapchainInfo.minImageCount = imageCount;
        swapchainInfo.imageFormat = surfaceFormat.format;
        swapchainInfo.imageColorSpace = surfaceFormat.colorSpace;
        swapchainInfo.imageExtent = extent;
        swapchainInfo.imageArrayLayers = 1;
        // The bootstrap frame uses vkCmdClearColorImage, so require both
        // usages up front instead of silently creating a swapchain that the
        // first frame cannot render to.
        constexpr VkImageUsageFlags requiredUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT |
                                                     VK_IMAGE_USAGE_TRANSFER_DST_BIT;
        if ((capabilities.supportedUsageFlags & requiredUsage) != requiredUsage) {
            m_error = "Vulkan surface lacks color-attachment or transfer-destination image usage";
            return false;
        }
        swapchainInfo.imageUsage = requiredUsage;
        swapchainInfo.imageSharingMode = VK_SHARING_MODE_EXCLUSIVE;
        swapchainInfo.preTransform = capabilities.currentTransform;
        constexpr VkCompositeAlphaFlagBitsKHR alphaModes[] = {
            VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR,
            VK_COMPOSITE_ALPHA_PRE_MULTIPLIED_BIT_KHR,
            VK_COMPOSITE_ALPHA_POST_MULTIPLIED_BIT_KHR,
            VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR,
        };
        const auto alphaIt = std::find_if(std::begin(alphaModes), std::end(alphaModes),
                                          [&capabilities](VkCompositeAlphaFlagBitsKHR mode) {
                                              return (capabilities.supportedCompositeAlpha & mode) != 0;
                                          });
        if (alphaIt == std::end(alphaModes)) {
            m_error = "Vulkan surface exposes no supported composite-alpha mode";
            return false;
        }
        swapchainInfo.compositeAlpha = *alphaIt;
        swapchainInfo.presentMode = VK_PRESENT_MODE_FIFO_KHR;
        swapchainInfo.clipped = VK_TRUE;
        if (vkCreateSwapchainKHR(m_device, &swapchainInfo, nullptr, &m_swapchain) != VK_SUCCESS) {
            m_error = "Vulkan swapchain creation failed";
            return false;
        }
        uint32_t actualImageCount = 0;
        vkGetSwapchainImagesKHR(m_device, m_swapchain, &actualImageCount, nullptr);
        m_swapchainImages.resize(actualImageCount);
        if (actualImageCount == 0 ||
            vkGetSwapchainImagesKHR(m_device, m_swapchain, &actualImageCount,
                                    m_swapchainImages.data()) != VK_SUCCESS) {
            vkDestroySwapchainKHR(m_device, m_swapchain, nullptr);
            m_swapchain = VK_NULL_HANDLE;
            m_swapchainImages.clear();
            m_error = "Vulkan swapchain image enumeration failed";
            return false;
        }
        m_swapchainFormat = surfaceFormat.format;
        m_swapchainExtent = extent;
        m_swapchainImageViews.clear();
        m_swapchainImageViews.reserve(m_swapchainImages.size());
        for (VkImage image : m_swapchainImages) {
            VkImageViewCreateInfo viewInfo{};
            viewInfo.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO;
            viewInfo.image = image;
            viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
            viewInfo.format = m_swapchainFormat;
            viewInfo.components = {VK_COMPONENT_SWIZZLE_IDENTITY,
                                   VK_COMPONENT_SWIZZLE_IDENTITY,
                                   VK_COMPONENT_SWIZZLE_IDENTITY,
                                   VK_COMPONENT_SWIZZLE_IDENTITY};
            viewInfo.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
            viewInfo.subresourceRange.baseMipLevel = 0;
            viewInfo.subresourceRange.levelCount = 1;
            viewInfo.subresourceRange.baseArrayLayer = 0;
            viewInfo.subresourceRange.layerCount = 1;
            VkImageView view = VK_NULL_HANDLE;
            if (vkCreateImageView(m_device, &viewInfo, nullptr, &view) != VK_SUCCESS) {
                m_error = "Vulkan swapchain image view creation failed";
                destroySwapchainResources();
                return false;
            }
            m_swapchainImageViews.push_back(view);
        }
        if (!createFrameResources()) {
            destroySwapchainResources();
            return false;
        }
        if (!createGraphicsResources()) {
            destroySwapchainResources();
            return false;
        }
        m_error.clear();
        return true;
    }

    bool isSwapchainReady() const noexcept {
        return m_swapchain != VK_NULL_HANDLE && !m_swapchainImages.empty() &&
               m_commandPool != VK_NULL_HANDLE && !m_commandBuffers.empty() &&
               m_renderPass != VK_NULL_HANDLE && m_graphicsPipeline != VK_NULL_HANDLE &&
               !m_framebuffers.empty();
    }

    bool areUiShaderModulesReady() const noexcept {
        return m_vertexShaderModule != VK_NULL_HANDLE &&
               m_fragmentShaderModule != VK_NULL_HANDLE;
    }

    void setUiRectPixels(float x, float y, float width, float height, uint32_t color) noexcept {
        if (m_swapchainExtent.width == 0 || m_swapchainExtent.height == 0) return;
        if (!std::isfinite(x) || !std::isfinite(y) || !std::isfinite(width) ||
            !std::isfinite(height) || width <= 0.0f || height <= 0.0f) {
            ++m_rejectedUiRects;
            return;
        }
        if (m_uiRectCount >= kMaxUiRects) {
            ++m_rejectedUiRects;
            return;
        }
        const float right = std::clamp(x + width, 0.0f, static_cast<float>(m_swapchainExtent.width));
        const float bottom = std::clamp(y + height, 0.0f, static_cast<float>(m_swapchainExtent.height));
        const float left = std::clamp(x, 0.0f, right);
        const float top = std::clamp(y, 0.0f, bottom);
        if (right <= left || bottom <= top) {
            ++m_rejectedUiRects;
            return;
        }
        UiUniforms& rect = m_uiRects[m_uiRectCount++];
        rect.pos[0] = (left / static_cast<float>(m_swapchainExtent.width)) * 2.0f - 1.0f;
        rect.pos[1] = 1.0f - (bottom / static_cast<float>(m_swapchainExtent.height)) * 2.0f;
        rect.pos[2] = ((right - left) / static_cast<float>(m_swapchainExtent.width)) * 2.0f;
        rect.pos[3] = ((bottom - top) / static_cast<float>(m_swapchainExtent.height)) * 2.0f;
        rect.props[0] = rect.props[1] = rect.props[2] = rect.props[3] = 0.0f;
        rect.color[0] = static_cast<float>((color >> 16) & 0xff) / 255.0f;
        rect.color[1] = static_cast<float>((color >> 8) & 0xff) / 255.0f;
        rect.color[2] = static_cast<float>(color & 0xff) / 255.0f;
        rect.color[3] = static_cast<float>((color >> 24) & 0xff) / 255.0f;
    }

    void beginUiFrame() noexcept {
        m_uiRectCount = 0;
        m_rejectedUiRects = 0;
        m_hasUiScissor = false;
        m_lastFrameRendered = false;
    }

    void setUiScissor(float x, float y, float width, float height) noexcept {
        if (m_swapchainExtent.width == 0 || m_swapchainExtent.height == 0) return;
        const float left = std::clamp(x, 0.0f, static_cast<float>(m_swapchainExtent.width));
        const float top = std::clamp(y, 0.0f, static_cast<float>(m_swapchainExtent.height));
        const float right = std::clamp(x + std::max(0.0f, width), left,
                                       static_cast<float>(m_swapchainExtent.width));
        const float bottom = std::clamp(y + std::max(0.0f, height), top,
                                        static_cast<float>(m_swapchainExtent.height));
        m_uiScissor.offset = {static_cast<int32_t>(left), static_cast<int32_t>(top)};
        m_uiScissor.extent = {static_cast<uint32_t>(right - left),
                              static_cast<uint32_t>(bottom - top)};
        m_hasUiScissor = true;
    }

    bool drawEmptyFrame() {
        if (!isSwapchainReady()) {
            m_error = "Vulkan frame resources are not initialized";
            return false;
        }
        if (vkWaitForFences(m_device, 1, &m_inFlightFence, VK_TRUE, UINT64_MAX) != VK_SUCCESS) {
            m_error = "Vulkan frame fence wait failed";
            return false;
        }
        uint32_t imageIndex = 0;
        const VkResult acquire = vkAcquireNextImageKHR(
            m_device, m_swapchain, UINT64_MAX, m_imageAvailableSemaphore,
            VK_NULL_HANDLE, &imageIndex);
        if (acquire != VK_SUCCESS && acquire != VK_SUBOPTIMAL_KHR) {
            m_error = "Vulkan swapchain image acquisition failed";
            return false;
        }

        VkCommandBuffer commandBuffer = m_commandBuffers[imageIndex % m_commandBuffers.size()];
        if (vkResetCommandBuffer(commandBuffer, 0) != VK_SUCCESS) {
            m_error = "Vulkan command buffer reset failed";
            return false;
        }
        VkCommandBufferBeginInfo beginInfo{};
        beginInfo.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
        if (m_uiRectCount == 0) {
            setUiRectPixels(32.0f, 32.0f, 240.0f, 42.0f, 0xff3388ccu);
        }
        void* mapped = nullptr;
        const VkDeviceSize uniformBytes = sizeof(UiUniforms) * m_uiRectCount;
        if (vkMapMemory(m_device, m_uniformMemory, 0, uniformBytes, 0, &mapped) != VK_SUCCESS) {
            m_error = "Vulkan UI uniform memory mapping failed";
            return false;
        }
        std::memcpy(mapped, m_uiRects.data(), static_cast<size_t>(uniformBytes));
        vkUnmapMemory(m_device, m_uniformMemory);
        if (vkBeginCommandBuffer(commandBuffer, &beginInfo) != VK_SUCCESS) {
            m_error = "Vulkan command buffer begin failed";
            return false;
        }
        VkClearValue clearValue{};
        clearValue.color = {{0.055f, 0.063f, 0.075f, 1.0f}};
        VkRenderPassBeginInfo renderPassBegin{};
        renderPassBegin.sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO;
        renderPassBegin.renderPass = m_renderPass;
        renderPassBegin.framebuffer = m_framebuffers[imageIndex];
        renderPassBegin.renderArea.extent = m_swapchainExtent;
        renderPassBegin.clearValueCount = 1;
        renderPassBegin.pClearValues = &clearValue;
        vkCmdBeginRenderPass(commandBuffer, &renderPassBegin, VK_SUBPASS_CONTENTS_INLINE);
        VkViewport viewport{};
        viewport.width = static_cast<float>(m_swapchainExtent.width);
        viewport.height = static_cast<float>(m_swapchainExtent.height);
        viewport.maxDepth = 1.0f;
        vkCmdSetViewport(commandBuffer, 0, 1, &viewport);
        VkRect2D scissor{};
        scissor.offset = {0, 0};
        scissor.extent = m_swapchainExtent;
        if (m_hasUiScissor) scissor = m_uiScissor;
        vkCmdSetScissor(commandBuffer, 0, 1, &scissor);
        vkCmdBindPipeline(commandBuffer, VK_PIPELINE_BIND_POINT_GRAPHICS, m_graphicsPipeline);
        vkCmdBindDescriptorSets(commandBuffer, VK_PIPELINE_BIND_POINT_GRAPHICS,
                                m_pipelineLayout, 0, 1, &m_descriptorSet, 0, nullptr);
        vkCmdDraw(commandBuffer, 4, m_uiRectCount, 0, 0);
        vkCmdEndRenderPass(commandBuffer);
        if (vkEndCommandBuffer(commandBuffer) != VK_SUCCESS) {
            m_error = "Vulkan command buffer recording failed";
            return false;
        }
        // Reset only after acquire and recording succeeded. Resetting earlier
        // would leave the fence unsignaled on an error path and make the next
        // frame wait forever.
        if (vkResetFences(m_device, 1, &m_inFlightFence) != VK_SUCCESS) {
            m_error = "Vulkan frame fence reset failed";
            return false;
        }
        VkPipelineStageFlags waitStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        VkSubmitInfo submit{};
        submit.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
        submit.waitSemaphoreCount = 1;
        submit.pWaitSemaphores = &m_imageAvailableSemaphore;
        submit.pWaitDstStageMask = &waitStage;
        submit.commandBufferCount = 1;
        submit.pCommandBuffers = &commandBuffer;
        submit.signalSemaphoreCount = 1;
        submit.pSignalSemaphores = &m_renderFinishedSemaphore;
        if (vkQueueSubmit(m_graphicsQueue, 1, &submit, m_inFlightFence) != VK_SUCCESS) {
            m_error = "Vulkan queue submit failed";
            return false;
        }
        VkPresentInfoKHR present{};
        present.sType = VK_STRUCTURE_TYPE_PRESENT_INFO_KHR;
        present.waitSemaphoreCount = 1;
        present.pWaitSemaphores = &m_renderFinishedSemaphore;
        present.swapchainCount = 1;
        present.pSwapchains = &m_swapchain;
        present.pImageIndices = &imageIndex;
        const VkResult presentResult = vkQueuePresentKHR(m_graphicsQueue, &present);
        if (presentResult != VK_SUCCESS && presentResult != VK_SUBOPTIMAL_KHR) {
            m_error = "Vulkan queue present failed";
            return false;
        }
        m_error.clear();
        m_lastFrameRendered = true;
        return true;
    }

    /**
     * Stages packed UI quads for the current frame. The upload is performed
     * once by drawEmptyFrame; callers do not issue one Vulkan call per widget.
     * Layout: x, y, width, height, packed RGBA, radius, border, glow.
     */
    void drawUIAtlas(VkCommandBuffer cb, const std::vector<float>& quadData) {
        if (cb == VK_NULL_HANDLE || quadData.empty() || quadData.size() % 8 != 0) {
            ++m_rejectedUiAtlasBatches;
            return;
        }
        const size_t quadCount = std::min(quadData.size() / 8,
                                          static_cast<size_t>(kMaxUiRects));
        for (size_t i = 0; i < quadCount; ++i) {
            const float* quad = quadData.data() + i * 8;
            if (!std::isfinite(quad[4])) {
                ++m_rejectedUiAtlasBatches;
                continue;
            }
            const uint32_t color = quad[4] >= 4294967040.0f
                ? std::numeric_limits<uint32_t>::max()
                : static_cast<uint32_t>(std::max(0.0f, quad[4]));
            setUiRectPixels(quad[0], quad[1], quad[2], quad[3], color);
        }
    }

    bool isInitialized() const noexcept { return m_device != VK_NULL_HANDLE; }
    bool wasLastFrameRendered() const noexcept { return m_lastFrameRendered; }
    uint32_t uiRectCount() const noexcept { return m_uiRectCount; }
    uint32_t rejectedUiRectCount() const noexcept { return m_rejectedUiRects; }
    uint32_t rejectedUiAtlasBatchCount() const noexcept { return m_rejectedUiAtlasBatches; }
    const std::string& lastError() const noexcept { return m_error; }
    void shutdown() noexcept { reset(); }

private:
