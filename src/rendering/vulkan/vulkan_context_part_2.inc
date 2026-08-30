    VulkanContext() = default;

    bool createFrameResources() {
        VkCommandPoolCreateInfo poolInfo{};
        poolInfo.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO;
        poolInfo.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
        poolInfo.queueFamilyIndex = m_graphicsQueueFamily;
        if (vkCreateCommandPool(m_device, &poolInfo, nullptr, &m_commandPool) != VK_SUCCESS) {
            m_error = "Vulkan command pool creation failed";
            return false;
        }
        m_commandBuffers.resize(m_swapchainImages.size());
        VkCommandBufferAllocateInfo allocation{};
        allocation.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
        allocation.commandPool = m_commandPool;
        allocation.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
        allocation.commandBufferCount = static_cast<uint32_t>(m_commandBuffers.size());
        if (vkAllocateCommandBuffers(m_device, &allocation, m_commandBuffers.data()) != VK_SUCCESS) {
            m_error = "Vulkan command buffer allocation failed";
            return false;
        }
        VkSemaphoreCreateInfo semaphoreInfo{};
        semaphoreInfo.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO;
        if (vkCreateSemaphore(m_device, &semaphoreInfo, nullptr, &m_imageAvailableSemaphore) != VK_SUCCESS ||
            vkCreateSemaphore(m_device, &semaphoreInfo, nullptr, &m_renderFinishedSemaphore) != VK_SUCCESS) {
            m_error = "Vulkan semaphore creation failed";
            return false;
        }
        VkFenceCreateInfo fenceInfo{};
        fenceInfo.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO;
        fenceInfo.flags = VK_FENCE_CREATE_SIGNALED_BIT;
        if (vkCreateFence(m_device, &fenceInfo, nullptr, &m_inFlightFence) != VK_SUCCESS) {
            m_error = "Vulkan fence creation failed";
            return false;
        }
        return true;
    }

    uint32_t findMemoryType(uint32_t typeBits, VkMemoryPropertyFlags properties) const {
        VkPhysicalDeviceMemoryProperties memoryProperties{};
        vkGetPhysicalDeviceMemoryProperties(m_physicalDevice, &memoryProperties);
        for (uint32_t index = 0; index < memoryProperties.memoryTypeCount; ++index) {
            if ((typeBits & (1u << index)) != 0 &&
                (memoryProperties.memoryTypes[index].propertyFlags & properties) == properties) {
                return index;
            }
        }
        return std::numeric_limits<uint32_t>::max();
    }

    bool createGraphicsResources() {
        VkAttachmentDescription attachment{};
        attachment.format = m_swapchainFormat;
        attachment.samples = VK_SAMPLE_COUNT_1_BIT;
        attachment.loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR;
        attachment.storeOp = VK_ATTACHMENT_STORE_OP_STORE;
        attachment.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        attachment.finalLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
        VkAttachmentReference colorReference{};
        colorReference.attachment = 0;
        colorReference.layout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
        VkSubpassDescription subpass{};
        subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
        subpass.colorAttachmentCount = 1;
        subpass.pColorAttachments = &colorReference;
        VkSubpassDependency dependency{};
        dependency.srcSubpass = VK_SUBPASS_EXTERNAL;
        dependency.dstSubpass = 0;
        dependency.srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        dependency.dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        dependency.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
        VkRenderPassCreateInfo renderPassInfo{};
        renderPassInfo.sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO;
        renderPassInfo.attachmentCount = 1;
        renderPassInfo.pAttachments = &attachment;
        renderPassInfo.subpassCount = 1;
        renderPassInfo.pSubpasses = &subpass;
        renderPassInfo.dependencyCount = 1;
        renderPassInfo.pDependencies = &dependency;
        if (vkCreateRenderPass(m_device, &renderPassInfo, nullptr, &m_renderPass) != VK_SUCCESS) {
            m_error = "Vulkan render pass creation failed";
            return false;
        }

        m_framebuffers.resize(m_swapchainImageViews.size(), VK_NULL_HANDLE);
        for (size_t index = 0; index < m_swapchainImageViews.size(); ++index) {
            VkFramebufferCreateInfo framebufferInfo{};
            framebufferInfo.sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO;
            framebufferInfo.renderPass = m_renderPass;
            framebufferInfo.attachmentCount = 1;
            framebufferInfo.pAttachments = &m_swapchainImageViews[index];
            framebufferInfo.width = m_swapchainExtent.width;
            framebufferInfo.height = m_swapchainExtent.height;
            framebufferInfo.layers = 1;
            if (vkCreateFramebuffer(m_device, &framebufferInfo, nullptr,
                                    &m_framebuffers[index]) != VK_SUCCESS) {
                m_error = "Vulkan framebuffer creation failed";
                return false;
            }
        }

        VkDescriptorSetLayoutBinding uniformBinding{};
        uniformBinding.binding = 0;
        uniformBinding.descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
        uniformBinding.descriptorCount = 1;
        uniformBinding.stageFlags = VK_SHADER_STAGE_VERTEX_BIT;
        VkDescriptorSetLayoutCreateInfo descriptorLayoutInfo{};
        descriptorLayoutInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
        descriptorLayoutInfo.bindingCount = 1;
        descriptorLayoutInfo.pBindings = &uniformBinding;
        if (vkCreateDescriptorSetLayout(m_device, &descriptorLayoutInfo, nullptr,
                                         &m_descriptorSetLayout) != VK_SUCCESS) {
            m_error = "Vulkan descriptor-set layout creation failed";
            return false;
        }

        VkPipelineLayoutCreateInfo pipelineLayoutInfo{};
        pipelineLayoutInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO;
        pipelineLayoutInfo.setLayoutCount = 1;
        pipelineLayoutInfo.pSetLayouts = &m_descriptorSetLayout;
        if (vkCreatePipelineLayout(m_device, &pipelineLayoutInfo, nullptr,
                                    &m_pipelineLayout) != VK_SUCCESS) {
            m_error = "Vulkan pipeline layout creation failed";
            return false;
        }

        VkBufferCreateInfo bufferInfo{};
        bufferInfo.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO;
        bufferInfo.size = sizeof(UiUniforms) * kMaxUiRects;
        bufferInfo.usage = VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT;
        bufferInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
        if (vkCreateBuffer(m_device, &bufferInfo, nullptr, &m_uniformBuffer) != VK_SUCCESS) {
            m_error = "Vulkan UI uniform buffer creation failed";
            return false;
        }
        VkMemoryRequirements memoryRequirements{};
        vkGetBufferMemoryRequirements(m_device, m_uniformBuffer, &memoryRequirements);
        const uint32_t memoryType = findMemoryType(
            memoryRequirements.memoryTypeBits,
            VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
        if (memoryType == std::numeric_limits<uint32_t>::max()) {
            m_error = "Vulkan host-visible uniform memory type not found";
            return false;
        }
        VkMemoryAllocateInfo allocationInfo{};
        allocationInfo.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
        allocationInfo.allocationSize = memoryRequirements.size;
        allocationInfo.memoryTypeIndex = memoryType;
        if (vkAllocateMemory(m_device, &allocationInfo, nullptr, &m_uniformMemory) != VK_SUCCESS ||
            vkBindBufferMemory(m_device, m_uniformBuffer, m_uniformMemory, 0) != VK_SUCCESS) {
            m_error = "Vulkan UI uniform memory allocation failed";
            return false;
        }
        VkDescriptorPoolSize poolSize{};
        poolSize.type = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
        poolSize.descriptorCount = 1;
        VkDescriptorPoolCreateInfo poolInfo{};
        poolInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO;
        poolInfo.maxSets = 1;
        poolInfo.poolSizeCount = 1;
        poolInfo.pPoolSizes = &poolSize;
        if (vkCreateDescriptorPool(m_device, &poolInfo, nullptr, &m_descriptorPool) != VK_SUCCESS) {
            m_error = "Vulkan descriptor pool creation failed";
            return false;
        }
        VkDescriptorSetAllocateInfo setInfo{};
        setInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
        setInfo.descriptorPool = m_descriptorPool;
        setInfo.descriptorSetCount = 1;
        setInfo.pSetLayouts = &m_descriptorSetLayout;
        if (vkAllocateDescriptorSets(m_device, &setInfo, &m_descriptorSet) != VK_SUCCESS) {
            m_error = "Vulkan descriptor set allocation failed";
            return false;
        }
        VkDescriptorBufferInfo descriptorBuffer{};
        descriptorBuffer.buffer = m_uniformBuffer;
        descriptorBuffer.range = sizeof(UiUniforms) * kMaxUiRects;
        VkWriteDescriptorSet write{};
        write.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        write.dstSet = m_descriptorSet;
        write.dstBinding = 0;
        write.descriptorCount = 1;
        write.descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
        write.pBufferInfo = &descriptorBuffer;
        vkUpdateDescriptorSets(m_device, 1, &write, 0, nullptr);

        VkPipelineShaderStageCreateInfo stages[2]{};
        stages[0].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
        stages[0].stage = VK_SHADER_STAGE_VERTEX_BIT;
        stages[0].module = m_vertexShaderModule;
        stages[0].pName = "main";
        stages[1] = stages[0];
        stages[1].stage = VK_SHADER_STAGE_FRAGMENT_BIT;
        stages[1].module = m_fragmentShaderModule;
        VkPipelineVertexInputStateCreateInfo vertexInput{};
        vertexInput.sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO;
        VkPipelineInputAssemblyStateCreateInfo assembly{};
        assembly.sType = VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO;
        assembly.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_STRIP;
        VkPipelineViewportStateCreateInfo viewport{};
        viewport.sType = VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO;
        viewport.viewportCount = 1;
        viewport.scissorCount = 1;
        VkPipelineRasterizationStateCreateInfo raster{};
        raster.sType = VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO;
        raster.polygonMode = VK_POLYGON_MODE_FILL;
        raster.cullMode = VK_CULL_MODE_NONE;
        raster.frontFace = VK_FRONT_FACE_COUNTER_CLOCKWISE;
        raster.lineWidth = 1.0f;
        VkPipelineMultisampleStateCreateInfo multisample{};
        multisample.sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO;
        multisample.rasterizationSamples = VK_SAMPLE_COUNT_1_BIT;
        VkPipelineColorBlendAttachmentState blendAttachment{};
        blendAttachment.blendEnable = VK_TRUE;
        blendAttachment.srcColorBlendFactor = VK_BLEND_FACTOR_SRC_ALPHA;
        blendAttachment.dstColorBlendFactor = VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA;
        blendAttachment.colorBlendOp = VK_BLEND_OP_ADD;
        blendAttachment.srcAlphaBlendFactor = VK_BLEND_FACTOR_ONE;
        blendAttachment.dstAlphaBlendFactor = VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA;
        blendAttachment.alphaBlendOp = VK_BLEND_OP_ADD;
        blendAttachment.colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT |
                                         VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT;
        VkPipelineColorBlendStateCreateInfo blend{};
        blend.sType = VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO;
        blend.attachmentCount = 1;
        blend.pAttachments = &blendAttachment;
        constexpr VkDynamicState dynamicStates[] = {VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR};
        VkPipelineDynamicStateCreateInfo dynamic{};
        dynamic.sType = VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO;
        dynamic.dynamicStateCount = 2;
        dynamic.pDynamicStates = dynamicStates;
        VkGraphicsPipelineCreateInfo pipelineInfo{};
        pipelineInfo.sType = VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO;
        pipelineInfo.stageCount = 2;
        pipelineInfo.pStages = stages;
        pipelineInfo.pVertexInputState = &vertexInput;
        pipelineInfo.pInputAssemblyState = &assembly;
        pipelineInfo.pViewportState = &viewport;
        pipelineInfo.pRasterizationState = &raster;
        pipelineInfo.pMultisampleState = &multisample;
        pipelineInfo.pColorBlendState = &blend;
        pipelineInfo.pDynamicState = &dynamic;
        pipelineInfo.layout = m_pipelineLayout;
        pipelineInfo.renderPass = m_renderPass;
        pipelineInfo.subpass = 0;
        if (vkCreateGraphicsPipelines(m_device, VK_NULL_HANDLE, 1, &pipelineInfo, nullptr,
                                      &m_graphicsPipeline) != VK_SUCCESS) {
            m_error = "Vulkan UI graphics pipeline creation failed";
            return false;
        }
        return true;
    }

    bool createUiShaderModules() {
#if AURA_HAS_EMBEDDED_VULKAN_UI_SPV
        VkShaderModuleCreateInfo vertexInfo{};
        vertexInfo.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO;
        vertexInfo.codeSize = sizeof(Aura::Graphics::VulkanShaders::kVertex);
        vertexInfo.pCode = Aura::Graphics::VulkanShaders::kVertex;
        if (vkCreateShaderModule(m_device, &vertexInfo, nullptr, &m_vertexShaderModule) != VK_SUCCESS) {
            m_error = "Vulkan vertex shader module creation failed";
            return false;
        }

        VkShaderModuleCreateInfo fragmentInfo{};
        fragmentInfo.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO;
        fragmentInfo.codeSize = sizeof(Aura::Graphics::VulkanShaders::kFragment);
        fragmentInfo.pCode = Aura::Graphics::VulkanShaders::kFragment;
        if (vkCreateShaderModule(m_device, &fragmentInfo, nullptr, &m_fragmentShaderModule) != VK_SUCCESS) {
            m_error = "Vulkan fragment shader module creation failed";
            vkDestroyShaderModule(m_device, m_vertexShaderModule, nullptr);
            m_vertexShaderModule = VK_NULL_HANDLE;
            return false;
        }
        return true;
#else
        m_error = "Embedded Vulkan UI SPIR-V is unavailable; enable Vulkan shader build validation";
        return false;
#endif
    }

    void destroySwapchainResources() noexcept {
        if (m_device != VK_NULL_HANDLE) {
            if (m_graphicsPipeline != VK_NULL_HANDLE)
                vkDestroyPipeline(m_device, m_graphicsPipeline, nullptr);
            if (m_uniformBuffer != VK_NULL_HANDLE)
                vkDestroyBuffer(m_device, m_uniformBuffer, nullptr);
            if (m_uniformMemory != VK_NULL_HANDLE)
                vkFreeMemory(m_device, m_uniformMemory, nullptr);
            if (m_descriptorPool != VK_NULL_HANDLE)
                vkDestroyDescriptorPool(m_device, m_descriptorPool, nullptr);
            if (m_pipelineLayout != VK_NULL_HANDLE)
                vkDestroyPipelineLayout(m_device, m_pipelineLayout, nullptr);
            if (m_descriptorSetLayout != VK_NULL_HANDLE)
                vkDestroyDescriptorSetLayout(m_device, m_descriptorSetLayout, nullptr);
            for (VkFramebuffer framebuffer : m_framebuffers)
                if (framebuffer != VK_NULL_HANDLE) vkDestroyFramebuffer(m_device, framebuffer, nullptr);
            if (m_renderPass != VK_NULL_HANDLE)
                vkDestroyRenderPass(m_device, m_renderPass, nullptr);
            if (m_inFlightFence != VK_NULL_HANDLE) vkDestroyFence(m_device, m_inFlightFence, nullptr);
            if (m_imageAvailableSemaphore != VK_NULL_HANDLE) vkDestroySemaphore(m_device, m_imageAvailableSemaphore, nullptr);
            if (m_renderFinishedSemaphore != VK_NULL_HANDLE) vkDestroySemaphore(m_device, m_renderFinishedSemaphore, nullptr);
            if (m_commandPool != VK_NULL_HANDLE) vkDestroyCommandPool(m_device, m_commandPool, nullptr);
            for (VkImageView view : m_swapchainImageViews)
                if (view != VK_NULL_HANDLE) vkDestroyImageView(m_device, view, nullptr);
        }
        m_inFlightFence = VK_NULL_HANDLE;
        m_vertexShaderModule = VK_NULL_HANDLE;
        m_fragmentShaderModule = VK_NULL_HANDLE;
        m_graphicsPipeline = VK_NULL_HANDLE;
        m_pipelineLayout = VK_NULL_HANDLE;
        m_renderPass = VK_NULL_HANDLE;
        m_framebuffers.clear();
        m_descriptorSetLayout = VK_NULL_HANDLE;
        m_descriptorPool = VK_NULL_HANDLE;
        m_descriptorSet = VK_NULL_HANDLE;
        m_uniformBuffer = VK_NULL_HANDLE;
        m_uniformMemory = VK_NULL_HANDLE;
        m_imageAvailableSemaphore = VK_NULL_HANDLE;
        m_renderFinishedSemaphore = VK_NULL_HANDLE;
        m_commandPool = VK_NULL_HANDLE;
        m_commandBuffers.clear();
        m_swapchainImageViews.clear();
    }

    void reset() noexcept {
        if (m_device != VK_NULL_HANDLE) {
            vkDeviceWaitIdle(m_device);
            destroySwapchainResources();
            if (m_vertexShaderModule != VK_NULL_HANDLE)
                vkDestroyShaderModule(m_device, m_vertexShaderModule, nullptr);
            if (m_fragmentShaderModule != VK_NULL_HANDLE)
                vkDestroyShaderModule(m_device, m_fragmentShaderModule, nullptr);
            m_vertexShaderModule = VK_NULL_HANDLE;
            m_fragmentShaderModule = VK_NULL_HANDLE;
        }
        if (m_device != VK_NULL_HANDLE && m_swapchain != VK_NULL_HANDLE)
            vkDestroySwapchainKHR(m_device, m_swapchain, nullptr);
        m_swapchain = VK_NULL_HANDLE;
        m_swapchainImages.clear();
        if (m_device != VK_NULL_HANDLE) {
            vkDestroyDevice(m_device, nullptr);
        }
        m_device = VK_NULL_HANDLE;
        m_graphicsQueue = VK_NULL_HANDLE;
        m_physicalDevice = VK_NULL_HANDLE;
        m_instance = VK_NULL_HANDLE;
        m_surface = VK_NULL_HANDLE;
        m_graphicsQueueFamily = std::numeric_limits<uint32_t>::max();
    }
    
    VkInstance m_instance = VK_NULL_HANDLE;
    VkSurfaceKHR m_surface = VK_NULL_HANDLE;
    VkPhysicalDevice m_physicalDevice = VK_NULL_HANDLE;
    VkDevice m_device = VK_NULL_HANDLE;
    VkQueue m_graphicsQueue = VK_NULL_HANDLE;
    VkSwapchainKHR m_swapchain = VK_NULL_HANDLE;
    std::vector<VkImage> m_swapchainImages;
    std::vector<VkImageView> m_swapchainImageViews;
    std::vector<VkCommandBuffer> m_commandBuffers;
    VkCommandPool m_commandPool = VK_NULL_HANDLE;
    VkSemaphore m_imageAvailableSemaphore = VK_NULL_HANDLE;
    VkSemaphore m_renderFinishedSemaphore = VK_NULL_HANDLE;
    VkFence m_inFlightFence = VK_NULL_HANDLE;
    VkShaderModule m_vertexShaderModule = VK_NULL_HANDLE;
    VkShaderModule m_fragmentShaderModule = VK_NULL_HANDLE;
    VkRenderPass m_renderPass = VK_NULL_HANDLE;
    std::vector<VkFramebuffer> m_framebuffers;
    VkPipeline m_graphicsPipeline = VK_NULL_HANDLE;
    VkPipelineLayout m_pipelineLayout = VK_NULL_HANDLE;
    VkDescriptorSetLayout m_descriptorSetLayout = VK_NULL_HANDLE;
    VkDescriptorPool m_descriptorPool = VK_NULL_HANDLE;
    VkDescriptorSet m_descriptorSet = VK_NULL_HANDLE;
    VkBuffer m_uniformBuffer = VK_NULL_HANDLE;
    VkDeviceMemory m_uniformMemory = VK_NULL_HANDLE;
    std::array<UiUniforms, kMaxUiRects> m_uiRects{};
    uint32_t m_uiRectCount = 0;
    uint32_t m_rejectedUiRects = 0;
    uint32_t m_rejectedUiAtlasBatches = 0;
    bool m_lastFrameRendered = false;
    VkFormat m_swapchainFormat = VK_FORMAT_UNDEFINED;
    VkExtent2D m_swapchainExtent{};
    uint32_t m_graphicsQueueFamily = std::numeric_limits<uint32_t>::max();
    std::string m_error;
    VkRect2D m_uiScissor{};
    bool m_hasUiScissor = false;
};
