
/**
 * @class VulkanGraphicsKernel
 * @brief High-performance Cross-platform Vulkan 1.3 Engine.
 */
VulkanGraphicsKernel::VulkanGraphicsKernel() {
    std::cout << "[Vulkan] Initializing Graphics Engine..." << std::endl;
}

VulkanGraphicsKernel::~VulkanGraphicsKernel() {
    cleanup();
}

bool VulkanGraphicsKernel::initialize(void* nativeWindowHandle) {
    cleanup();
    m_available = false;

    if (!nativeWindowHandle) {
        m_lastError = "Vulkan initialization requires a native window handle";
        return false;
    }

#if !defined(AURA_ENABLE_VULKAN) || !AURA_ENABLE_VULKAN
    (void)nativeWindowHandle;
    m_lastError = "Vulkan backend disabled at build time; use Metal or CPU fallback";
    return false;
#else
    // Runtime probing is deliberately not reported as renderer readiness.
    // A usable backend also needs a platform surface, swapchain, command
    // buffers, render pass, and pipeline; those are not owned by this class.
    uint32_t supportedApiVersion = VK_API_VERSION_1_0;
    const auto enumerateInstanceVersion = reinterpret_cast<PFN_vkEnumerateInstanceVersion>(
        vkGetInstanceProcAddr(VK_NULL_HANDLE, "vkEnumerateInstanceVersion"));
    if (enumerateInstanceVersion) {
        uint32_t queriedVersion = VK_API_VERSION_1_0;
        if (enumerateInstanceVersion(&queriedVersion) == VK_SUCCESS)
            supportedApiVersion = queriedVersion;
    }
    const uint32_t requestedApiVersion = std::min(supportedApiVersion, VK_API_VERSION_1_3);

    VkApplicationInfo appInfo{};
    appInfo.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO;
    appInfo.pApplicationName = "Tinja";
    appInfo.applicationVersion = VK_MAKE_VERSION(1, 0, 0);
    appInfo.pEngineName = "Aura Engine";
    appInfo.engineVersion = VK_MAKE_VERSION(1, 4, 0);
    appInfo.apiVersion = requestedApiVersion;

    VkInstanceCreateInfo createInfo{};
    createInfo.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO;
    createInfo.pApplicationInfo = &appInfo;

#if defined(__APPLE__)
    uint32_t instanceExtensionCount = 0;
    if (vkEnumerateInstanceExtensionProperties(nullptr, &instanceExtensionCount, nullptr) != VK_SUCCESS) {
        cleanup();
        m_lastError = "Vulkan instance extension enumeration failed";
        return false;
    }
    std::vector<VkExtensionProperties> instanceExtensions(instanceExtensionCount);
    vkEnumerateInstanceExtensionProperties(nullptr, &instanceExtensionCount, instanceExtensions.data());
    const auto hasInstanceExtension = [&instanceExtensions](const char* name) {
        return std::any_of(instanceExtensions.begin(), instanceExtensions.end(),
                           [name](const VkExtensionProperties& extension) {
                               return std::strcmp(extension.extensionName, name) == 0;
                           });
    };
    if (!hasInstanceExtension(VK_KHR_SURFACE_EXTENSION_NAME) ||
        !hasInstanceExtension(VK_EXT_METAL_SURFACE_EXTENSION_NAME)) {
        cleanup();
        m_lastError = "Vulkan MoltenVK surface extensions are unavailable";
        return false;
    }
    std::vector<const char*> enabledExtensions = {
        VK_KHR_SURFACE_EXTENSION_NAME,
        VK_EXT_METAL_SURFACE_EXTENSION_NAME,
    };
#ifdef VK_KHR_PORTABILITY_ENUMERATION_EXTENSION_NAME
    if (hasInstanceExtension(VK_KHR_PORTABILITY_ENUMERATION_EXTENSION_NAME)) {
        enabledExtensions.push_back(VK_KHR_PORTABILITY_ENUMERATION_EXTENSION_NAME);
        createInfo.flags |= VK_INSTANCE_CREATE_ENUMERATE_PORTABILITY_BIT_KHR;
    }
#endif
    createInfo.enabledExtensionCount = static_cast<uint32_t>(enabledExtensions.size());
    createInfo.ppEnabledExtensionNames = enabledExtensions.data();
#elif defined(_WIN32) || defined(VK_USE_PLATFORM_XLIB_KHR)
    uint32_t instanceExtensionCount = 0;
    if (vkEnumerateInstanceExtensionProperties(nullptr, &instanceExtensionCount, nullptr) != VK_SUCCESS) {
        cleanup();
        m_lastError = "Vulkan instance extension enumeration failed";
        return false;
    }
    std::vector<VkExtensionProperties> instanceExtensions(instanceExtensionCount);
    vkEnumerateInstanceExtensionProperties(nullptr, &instanceExtensionCount, instanceExtensions.data());
    const auto hasInstanceExtension = [&instanceExtensions](const char* name) {
        return std::any_of(instanceExtensions.begin(), instanceExtensions.end(),
                           [name](const VkExtensionProperties& extension) {
                               return std::strcmp(extension.extensionName, name) == 0;
                           });
    };
    std::vector<const char*> enabledExtensions;
    enabledExtensions.push_back(VK_KHR_SURFACE_EXTENSION_NAME);
#if defined(_WIN32)
    constexpr const char* platformSurface = VK_KHR_WIN32_SURFACE_EXTENSION_NAME;
#else
    constexpr const char* platformSurface = VK_KHR_XLIB_SURFACE_EXTENSION_NAME;
#endif
    if (!hasInstanceExtension(platformSurface)) {
        cleanup();
        m_lastError = "Vulkan native platform surface extension is unavailable";
        return false;
    }
    enabledExtensions.push_back(platformSurface);
    createInfo.enabledExtensionCount = static_cast<uint32_t>(enabledExtensions.size());
    createInfo.ppEnabledExtensionNames = enabledExtensions.data();
#endif

    const VkResult instanceResult = vkCreateInstance(&createInfo, nullptr, &m_instance);
    if (instanceResult != VK_SUCCESS) {
        cleanup();
        m_lastError = "Vulkan runtime or loader is unavailable";
        return false;
    }

#if defined(__APPLE__)
    if (!createVulkanSurfaceForNativeView(m_instance, nativeWindowHandle, &m_surface, m_lastError)) {
        cleanup();
        return false;
    }
    auto& context = Aura::Rendering::Vulkan::VulkanContext::getInstance();
    if (!context.initialize(m_instance, m_surface) || !context.createSwapchain(1280, 800)) {
        m_lastError = context.lastError();
        cleanup();
        return false;
    }
    m_contextReady = true;
    m_runtimeReady = true;
    m_available = true;
    m_lastError.clear();
    return true;
#else
    // Windows and Linux use the same context path as macOS once their
    // platform-specific surface has been created.  Keeping device selection
    // in VulkanContext avoids two competing queue/device owners and makes
    // swapchain readiness an actual capability rather than a probe result.
    if (!createVulkanSurfaceForNativeView(m_instance, nativeWindowHandle,
                                          &m_surface, m_lastError)) {
        cleanup();
        return false;
    }
    auto& context = Aura::Rendering::Vulkan::VulkanContext::getInstance();
    if (!context.initialize(m_instance, m_surface) ||
        !context.createSwapchain(1280, 800)) {
        m_lastError = context.lastError();
        cleanup();
        return false;
    }
    m_contextReady = true;
    m_runtimeReady = true;
    m_available = true;
    m_lastError.clear();
    return true;
#endif
#endif
}
