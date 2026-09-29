    // Heal a time/frequency selection by attenuating it with a smooth spectral
    // feather. This is intentionally bounded and click-free for offline edits.
    void healRegion(Core::AudioBuffer& buffer, double sampleRate, const Rect& target, float amount = 1.0f) {
        amount = std::clamp(std::isfinite(amount) ? amount : 1.0f, 0.0f, 1.0f);
        applySpectralGain(buffer, sampleRate, target, 1.0f - amount);
    }

    // Spectral inpainting: reconstruct a selected band from the nearest
    // unselected bins while retaining each bin's original phase. A short
    // Hann-windowed OLA keeps the repair continuous at the selection edges.
    void interpolateRegion(Core::AudioBuffer& buffer, double sampleRate,
                           Rect target, float blend = 1.0f) {
        if (buffer.getNumSamples() < 64 || buffer.getNumChannels() == 0 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 || sampleRate > 384000.0 ||
            !std::isfinite(target.t0) || !std::isfinite(target.t1) ||
            !std::isfinite(target.f0) || !std::isfinite(target.f1)) return;
        captureBefore(buffer, "Interpolate Region");
        std::vector<float*> channels;
        channels.reserve(buffer.getNumChannels());
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c)
            channels.push_back(buffer.getWritePointer(c));
        (void)hirari_spectral_processor_interpolate(channels.data(),
            static_cast<uint32_t>(channels.size()), buffer.getNumSamples(), sampleRate,
            target.t0, target.f0, target.t1, target.f1, blend);
    }

    // Detect isolated discontinuities and replace them with a short
    // equal-power interpolation. This is deliberately channel-local so a
    // click in one microphone does not alter the phase of the other channel.
    uint32_t removeClicks(Core::AudioBuffer& buffer, float threshold = 0.65f,
                          uint32_t radius = 8) {
        threshold = std::clamp(std::isfinite(threshold) ? threshold : 0.65f, 0.01f, 1.0f);
        radius = std::clamp<uint32_t>(radius, 1u, 64u);
        const uint32_t n = buffer.getNumSamples();
        if (n < radius * 2u + 1u) return 0;
        captureBefore(buffer, "Remove Clicks");
        std::vector<float*> channels;
        channels.reserve(buffer.getNumChannels());
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c)
            channels.push_back(buffer.getWritePointer(c));
        return hirari_spectral_processor_remove_clicks(channels.data(),
            static_cast<uint32_t>(channels.size()), n, 0.0, 0.0f, 0.0f,
            threshold, radius, false);
    }

    // Click removal constrained to a time/frequency canvas selection.  Clicks
    // are time-domain events, so the frequency bounds are intentionally
    // ignored; the time bounds are sample-accurate and the interpolation
    // window is clipped to the selected interval.
    uint32_t removeClicks(Core::AudioBuffer& buffer, double sampleRate,
                          Rect selection, float threshold = 0.65f,
                          uint32_t radius = 8) {
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() < 3 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 || sampleRate > 384000.0 ||
            !std::isfinite(selection.t0) || !std::isfinite(selection.t1)) return 0;
        threshold = std::clamp(std::isfinite(threshold) ? threshold : 0.65f, 0.01f, 1.0f);
        radius = std::clamp<uint32_t>(radius, 1u, 64u);
        const uint32_t n = buffer.getNumSamples();
        radius = std::min<uint32_t>(radius, (n - 1u) / 2u);
        const float duration = static_cast<float>(static_cast<double>(n) / sampleRate);
        selection.t0 = std::min(std::max(0.0f, std::min(selection.t0, selection.t1)), duration);
        selection.t1 = std::min(std::max(selection.t0, std::max(selection.t0, selection.t1)), duration);
        const uint32_t first = std::max<uint32_t>(radius, static_cast<uint32_t>(std::ceil(selection.t0 * sampleRate)));
        const uint64_t selectedLast = static_cast<uint64_t>(std::floor(selection.t1 * sampleRate));
        const uint32_t last = static_cast<uint32_t>(std::min<uint64_t>(n - radius - 1u, selectedLast));
        if (first > last) return 0;
        captureBefore(buffer, "Remove Clicks (Selection)");
        std::vector<float*> channels;
        channels.reserve(buffer.getNumChannels());
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c)
            channels.push_back(buffer.getWritePointer(c));
        return hirari_spectral_processor_remove_clicks(channels.data(),
            static_cast<uint32_t>(channels.size()), n, sampleRate, selection.t0,
            selection.t1, threshold, radius, true);
    }

    // Reconstruct runs that hit a digital ceiling. The surrounding samples
    // define the slope, while the run itself is filled with a smooth cubic
    // curve to avoid introducing a new discontinuity.
    uint32_t repairClipped(Core::AudioBuffer& buffer, float ceiling = 0.999f) {
        ceiling = std::clamp(std::isfinite(ceiling) ? std::fabs(ceiling) : 0.999f, 0.5f, 1.0f);
        const uint32_t n = buffer.getNumSamples();
        if (n > 0 && buffer.getNumChannels() > 0) captureBefore(buffer, "Repair Clipped");
        std::vector<float*> channels;
        channels.reserve(buffer.getNumChannels());
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c)
            channels.push_back(buffer.getWritePointer(c));
        return hirari_spectral_processor_repair_clipped(channels.data(),
            static_cast<uint32_t>(channels.size()), n, 0.0, 0.0f, 0.0f, ceiling, false);
    }

    // Repair clipped runs only inside a selected time interval.  A run is
    // accepted only when both its surrounding samples are also inside the
    // selection, preventing a surgical edit from changing adjacent audio.
    uint32_t repairClipped(Core::AudioBuffer& buffer, double sampleRate,
                           Rect selection, float ceiling = 0.999f) {
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() < 3 ||
            !std::isfinite(sampleRate) || sampleRate < 8000.0 || sampleRate > 384000.0 ||
            !std::isfinite(selection.t0) || !std::isfinite(selection.t1)) return 0;
        ceiling = std::clamp(std::isfinite(ceiling) ? std::fabs(ceiling) : 0.999f, 0.5f, 1.0f);
        const uint32_t n = buffer.getNumSamples();
        const float duration = static_cast<float>(static_cast<double>(n) / sampleRate);
        selection.t0 = std::min(std::max(0.0f, std::min(selection.t0, selection.t1)), duration);
        selection.t1 = std::min(std::max(selection.t0, std::max(selection.t0, selection.t1)), duration);
        const uint32_t first = std::max<uint32_t>(1u, static_cast<uint32_t>(std::ceil(selection.t0 * sampleRate)));
        const uint64_t selectedLast = static_cast<uint64_t>(std::floor(selection.t1 * sampleRate));
        const uint32_t last = static_cast<uint32_t>(std::min<uint64_t>(n - 2u, selectedLast));
        if (first > last) return 0;
        captureBefore(buffer, "Repair Clipped (Selection)");
        std::vector<float*> channels;
        channels.reserve(buffer.getNumChannels());
        for (uint32_t c = 0; c < buffer.getNumChannels(); ++c)
            channels.push_back(buffer.getWritePointer(c));
        return hirari_spectral_processor_repair_clipped(channels.data(),
            static_cast<uint32_t>(channels.size()), n, sampleRate, selection.t0,
            selection.t1, ceiling, true);
    }
