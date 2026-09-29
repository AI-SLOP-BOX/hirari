public:
    /// Bounded-memory IEEE-float recorder.  Unlike the offline writers, the
    /// final frame count is not known when the file is opened, so a fixed
    /// JUNK reservation is patched into a RIFF/RF64 header at finish time.
    /// Publication, durability, and temporary-file ownership remain exactly
    /// the same as every other canonical writer.
    class Float32StreamWriter {
    public:
        Float32StreamWriter(const std::string& path, uint32_t sampleRate,
                            uint16_t channels = 2) {
            m_handle = hirari_wav_float32_stream_create(
                path.data(), path.size(), sampleRate, channels);
            if (!m_handle) m_error = "Rust float32 stream writer could not be opened";
        }

        Float32StreamWriter(const Float32StreamWriter&) = delete;
        Float32StreamWriter& operator=(const Float32StreamWriter&) = delete;
        ~Float32StreamWriter() { hirari_wav_float32_stream_free(m_handle); }

        bool isOpen() const noexcept { return m_handle != nullptr && m_error.empty(); }
        const std::string& error() const noexcept { return m_error; }
        uint64_t framesWritten() const noexcept {
            return hirari_wav_float32_stream_frames(m_handle);
        }

        bool writeFrames(const float* const* channels, uint32_t count) {
            if (isOpen() && hirari_wav_float32_stream_write(m_handle, channels, count)) return true;
            if (m_error.empty()) m_error = "Rust float32 stream write failed";
            return false;
        }

        bool finish() {
            if (m_handle && hirari_wav_float32_stream_finish(m_handle)) return true;
            if (m_error.empty()) m_error = "Rust float32 stream publication failed";
            return false;
        }

    private:
        void* m_handle = nullptr;
        std::string m_error;
    };

    /// Streaming 24-bit PCM publisher used by offline renderers.  Header
    /// sizes are known up front, so the writer can keep bounded memory while
    /// sharing the canonical RF64, durability, and atomic-publish contract.
    class Pcm24StreamWriter {
    public:
        Pcm24StreamWriter(const std::string& path, uint64_t frames, uint32_t sampleRate,
                          bool broadcastWave = false, uint16_t channels = 2) {
            if (channels == 0 || channels > 2) {
                m_error = "PCM24 compatibility writer supports mono or stereo";
                return;
            }
            m_handle = hirari_wav_pcm_stream_create(path.data(), path.size(), frames,
                sampleRate, channels, 24, broadcastWave ? 1 : 0);
            if (!m_handle) m_error = "Rust PCM24 stream writer could not be opened";
        }
        Pcm24StreamWriter(const Pcm24StreamWriter&) = delete;
        Pcm24StreamWriter& operator=(const Pcm24StreamWriter&) = delete;
        ~Pcm24StreamWriter() { hirari_wav_pcm_stream_free(m_handle); }
        bool isOpen() const noexcept { return m_handle != nullptr && m_error.empty(); }
        const std::string& error() const noexcept { return m_error; }
        bool writeFrames(const float* left, const float* right, uint32_t count) {
            const float* planes[2] = {left, right};
            if (isOpen() && hirari_wav_pcm_stream_write(m_handle, planes, count)) return true;
            if (m_error.empty()) m_error = "Rust PCM24 stream write failed";
            return false;
        }
        bool finish() {
            if (m_handle && hirari_wav_pcm_stream_finish(m_handle)) return true;
            if (m_error.empty()) m_error = "Rust PCM24 stream publication failed";
            return false;
        }
    private:
        void* m_handle = nullptr;
        std::string m_error;
    };

    /// Compatibility streaming publisher for legacy stereo PCM16 callbacks.
    /// It intentionally shares the same publication lifecycle as PCM24 while
    /// preserving the old API's on-disk format.
    class Pcm16StreamWriter {
    public:
        Pcm16StreamWriter(const std::string& path, uint64_t frames, uint32_t sampleRate,
                          uint16_t channels = 2) {
            if (channels == 0 || channels > 2) {
                m_error = "PCM16 compatibility writer supports mono or stereo";
                return;
            }
            m_handle = hirari_wav_pcm_stream_create(path.data(), path.size(), frames,
                sampleRate, channels, 16, 0);
            if (!m_handle) m_error = "Rust PCM16 stream writer could not be opened";
        }
        Pcm16StreamWriter(const Pcm16StreamWriter&) = delete;
        Pcm16StreamWriter& operator=(const Pcm16StreamWriter&) = delete;
        ~Pcm16StreamWriter() { hirari_wav_pcm_stream_free(m_handle); }
        bool isOpen() const noexcept { return m_handle != nullptr && m_error.empty(); }
        const std::string& error() const noexcept { return m_error; }
        bool writeFrames(const float* left, const float* right, uint32_t count) {
            const float* planes[2] = {left, right};
            if (isOpen() && hirari_wav_pcm_stream_write(m_handle, planes, count)) return true;
            if (m_error.empty()) m_error = "Rust PCM16 stream write failed";
            return false;
        }
        bool finish() {
            if (m_handle && hirari_wav_pcm_stream_finish(m_handle)) return true;
            if (m_error.empty()) m_error = "Rust PCM16 stream publication failed";
            return false;
        }
    private:
        void* m_handle = nullptr;
        std::string m_error;
    };

    /// Bounded-memory WAVE64 IEEE-float publisher for long offline renders.
    /// The writer accepts one block at a time and only publishes after the
    /// complete payload has been flushed, synced, and atomically renamed.
    class Wave64FloatStreamWriter {
    public:
        Wave64FloatStreamWriter(const std::string& path, uint64_t frames,
                                uint32_t sampleRate, uint16_t channels = 2) {
            m_handle = hirari_wav_wave64_stream_create(
                path.data(), path.size(), frames, sampleRate, channels);
            if (!m_handle) m_error = "Rust WAVE64 stream writer could not be opened";
        }
        Wave64FloatStreamWriter(const Wave64FloatStreamWriter&) = delete;
        Wave64FloatStreamWriter& operator=(const Wave64FloatStreamWriter&) = delete;
        ~Wave64FloatStreamWriter() { hirari_wav_wave64_stream_free(m_handle); }
        bool isOpen() const noexcept { return m_handle != nullptr && m_error.empty(); }
        const std::string& error() const noexcept { return m_error; }
        uint64_t framesWritten() const noexcept {
            return hirari_wav_wave64_stream_frames(m_handle);
        }
        bool writeFrames(const float* const* channels, uint32_t count) {
            if (isOpen() && hirari_wav_wave64_stream_write(m_handle, channels, count)) return true;
            if (m_error.empty()) m_error = "Rust WAVE64 stream write failed";
            return false;
        }
        bool finish() {
            if (m_handle && hirari_wav_wave64_stream_finish(m_handle)) return true;
            if (m_error.empty()) m_error = "Rust WAVE64 stream publication failed";
            return false;
        }
    private:
        void* m_handle = nullptr;
        std::string m_error;
    };
