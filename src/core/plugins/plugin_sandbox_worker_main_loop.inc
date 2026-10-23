    #include "plugin_sandbox_worker_backend.inc"

#if !defined(_WIN32)
    hirari_clap_input_context_destroy(clapInputContext);
    if (clapInstance) hirari_clap_instance_destroy(clapInstance);
    if (pluginHandle != reinterpret_cast<void*>(1)) ::dlclose(pluginHandle);
#endif
    hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
    return 0;
}
