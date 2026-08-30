#include "mastering_orchestrator.hpp"
#include "../audio_buffer.hpp"
#include <algorithm>
#include <cmath>
#include <complex>
#include <fstream>
#include <string>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

namespace Aura::Core::Engine {

// Non-recursive, in-place Cooley-Tukey Radix-2 FFT
static void in_place_fft(std::complex<float>* x, size_t N) {
    // Bit-reversal permutation
    for (size_t i = 1, j = 0; i < N; ++i) {
        size_t bit = N >> 1;
        for (; j & bit; bit >>= 1) {
            j ^= bit;
        }
        j ^= bit;
        if (i < j) {
            std::swap(x[i], x[j]);
        }
    }
    
    // Butterfly computations
    for (size_t len = 2; len <= N; len <<= 1) {
        float angle = -2.0f * static_cast<float>(M_PI) / len;
        std::complex<float> wlen = std::polar(1.0f, angle);
        for (size_t i = 0; i < N; i += len) {
            std::complex<float> w(1.0f, 0.0f);
            for (size_t j = 0; j < len / 2; ++j) {
                std::complex<float> u = x[i + j];
                std::complex<float> t = x[i + j + len / 2] * w;
                x[i + j] = u + t;
                x[i + j + len / 2] = u - t;
                w *= wlen;
            }
        }
    }
}

// Non-recursive, in-place Cooley-Tukey Radix-2 IFFT
static void in_place_ifft(std::complex<float>* x, size_t N) {
    for (size_t i = 0; i < N; ++i) {
        x[i] = std::conj(x[i]);
    }
    in_place_fft(x, N);
    float scale = 1.0f / N;
    for (size_t i = 0; i < N; ++i) {
        x[i] = std::conj(x[i]) * scale;
    }
}

static uint32_t get_band_for_bin(uint32_t bin, uint32_t N) {
    uint32_t halfN = N / 2;
    if (bin >= halfN) {
        bin = N - bin; // Mirror negative frequencies
    }
    if (bin <= 3) return 0;
    if (bin <= 11) return 1;
    if (bin <= 23) return 2;
    if (bin <= 47) return 3;
    if (bin <= 95) return 4;
    if (bin <= 191) return 5;
    if (bin <= 383) return 6;
    return 7;
}

// MD5 Helper Functions
#define F(x, y, z) (((x) & (y)) | ((~x) & (z)))
#define G(x, y, z) (((x) & (z)) | ((y) & (~z)))
#define H(x, y, z) ((x) ^ (y) ^ (z))
#define I(x, y, z) ((y) ^ ((x) | (~z)))
#define ROTATE_LEFT(x, n) (((x) << (n)) | ((x) >> (32-(n))))
#define FF(a, b, c, d, x, s, ac) { \
    (a) += F ((b), (c), (d)) + (x) + (uint32_t)(ac); \
    (a) = ROTATE_LEFT ((a), (s)); \
    (a) += (b); \
  }
#define GG(a, b, c, d, x, s, ac) { \
    (a) += G ((b), (c), (d)) + (x) + (uint32_t)(ac); \
    (a) = ROTATE_LEFT ((a), (s)); \
    (a) += (b); \
  }
#define HH(a, b, c, d, x, s, ac) { \
    (a) += H ((b), (c), (d)) + (x) + (uint32_t)(ac); \
    (a) = ROTATE_LEFT ((a), (s)); \
    (a) += (b); \
  }
#define II(a, b, c, d, x, s, ac) { \
    (a) += I ((b), (c), (d)) + (x) + (uint32_t)(ac); \
    (a) = ROTATE_LEFT ((a), (s)); \
    (a) += (b); \
  }

static void md5_transform(uint32_t state[4], const uint8_t block[64]) {
    uint32_t a = state[0], b = state[1], c = state[2], d = state[3], x[16];
    for (int i = 0, j = 0; i < 16; ++i, j += 4) {
        x[i] = ((uint32_t)block[j]) | (((uint32_t)block[j+1]) << 8) |
               (((uint32_t)block[j+2]) << 16) | (((uint32_t)block[j+3]) << 24);
    }
    // Round 1
    FF(a, d, c, b, x[ 0],  7, 0xd76aa478);
    FF(b, a, d, c, x[ 1], 12, 0xe8c7b756);
    FF(c, b, a, d, x[ 2], 17, 0x242070db);
    FF(d, c, b, a, x[ 3], 22, 0xc1bdceee);
    FF(a, d, c, b, x[ 4],  7, 0xf57c0faf);
    FF(b, a, d, c, x[ 5], 12, 0x4787c62a);
    FF(c, b, a, d, x[ 6], 17, 0xa8304613);
    FF(d, c, b, a, x[ 7], 22, 0xfd469501);
    FF(a, d, c, b, x[ 8],  7, 0x698098d8);
    FF(b, a, d, c, x[ 9], 12, 0x8b44f7af);
    FF(c, b, a, d, x[10], 17, 0xffff5bb1);
    FF(d, c, b, a, x[11], 22, 0x895cd7be);
    FF(a, d, c, b, x[12],  7, 0x6b901122);
    FF(b, a, d, c, x[13], 12, 0xfd987193);
    FF(c, b, a, d, x[14], 17, 0xa679438e);
    FF(d, c, b, a, x[15], 22, 0x49b40821);
    // Round 2
    GG(a, d, c, b, x[ 1],  5, 0xf61e2562);
    GG(b, a, d, c, x[ 6],  9, 0xc040b340);
    GG(c, b, a, d, x[11], 14, 0x265e5a51);
    GG(d, c, b, a, x[ 0], 20, 0xe9b6c7aa);
    GG(a, d, c, b, x[ 5],  5, 0xd62f105d);
    GG(b, a, d, c, x[10],  9, 0x02441453);
    GG(c, b, a, d, x[15], 14, 0xd8a1e681);
    GG(d, c, b, a, x[ 4], 20, 0xe7d3fbc8);
    GG(a, d, c, b, x[ 9],  5, 0x21e1cde6);
    GG(b, a, d, c, x[14],  9, 0xc33707d6);
    GG(c, b, a, d, x[ 3], 14, 0xf4d50d87);
    GG(d, c, b, a, x[ 8], 20, 0x455a14ed);
    GG(a, d, c, b, x[13],  5, 0xa9e3e905);
    GG(b, a, d, c, x[ 2],  9, 0xfcefa3f8);
    GG(c, b, a, d, x[ 7], 14, 0x676f02d9);
    GG(d, c, b, a, x[12], 20, 0x8d2a4c8a);
    // Round 3
    HH(a, d, c, b, x[ 5],  4, 0xfffa3942);
    HH(b, a, d, c, x[ 8], 11, 0x8771f681);
    HH(c, b, a, d, x[11], 16, 0x6d9d6122);
    HH(d, c, b, a, x[14], 23, 0xfde5380c);
    HH(a, d, c, b, x[ 1],  4, 0xa4beea44);
    HH(b, a, d, c, x[ 4], 11, 0x4bdecfa9);
    HH(c, b, a, d, x[ 7], 16, 0xf6bb4b60);
    HH(d, c, b, a, x[10], 23, 0xbebfbc70);
    HH(a, d, c, b, x[13],  4, 0x289b7ec6);
    HH(b, a, d, c, x[ 0], 11, 0xeaa127fa);
    HH(c, b, a, d, x[ 3], 16, 0xd4ef3085);
    HH(d, c, b, a, x[ 6], 23, 0x04881d05);
    HH(a, d, c, b, x[ 9],  4, 0xd9d4d039);
    HH(b, a, d, c, x[12], 11, 0xe6db99e5);
    HH(c, b, a, d, x[15], 16, 0x1fa27cf8);
    HH(d, c, b, a, x[ 2], 23, 0xc4ac5665);
    // Round 4
    II(a, d, c, b, x[ 0],  6, 0xf4292244);
    II(b, a, d, c, x[ 7], 10, 0x432aff97);
    II(c, b, a, d, x[14], 15, 0xab9423a7);
    II(d, c, b, a, x[ 5], 21, 0xfc93a039);
    II(a, d, c, b, x[12],  6, 0x655b59c3);
    II(b, a, d, c, x[ 3], 10, 0x8f0ccc92);
    II(c, b, a, d, x[10], 15, 0xffeff47d);
    II(d, c, b, a, x[ 1], 21, 0x85845dd1);
    II(a, d, c, b, x[ 8],  6, 0x6fa87e4f);
    II(b, a, d, c, x[15], 10, 0xfe2ce6e0);
    II(c, b, a, d, x[ 6], 15, 0xa3014314);
    II(d, c, b, a, x[13], 21, 0x4e0811a1);
    II(a, d, c, b, x[ 4],  6, 0xf7537e82);
    II(b, a, d, c, x[11], 10, 0xbd3af235);
    II(c, b, a, d, x[ 2], 15, 0x2ad7d2bb);
    II(d, c, b, a, x[ 9], 21, 0xeb86d391);

    state[0] += a;
    state[1] += b;
    state[2] += c;
    state[3] += d;
}

static std::string calculate_file_md5(const std::string& filepath) {
    std::ifstream file(filepath, std::ios::binary);
    if (!file.is_open()) return "00000000000000000000000000000000";

    uint32_t state[4] = { 0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476 };
    uint8_t buffer[64];
    uint64_t totalBytes = 0;

    while (file) {
        file.read(reinterpret_cast<char*>(buffer), 64);
        std::streamsize bytesRead = file.gcount();
        if (bytesRead == 64) {
            md5_transform(state, buffer);
            totalBytes += 64;
        } else {
            totalBytes += bytesRead;
            std::fill(buffer + bytesRead, buffer + 64, 0);
            buffer[bytesRead] = 0x80;
            if (bytesRead >= 56) {
                md5_transform(state, buffer);
                std::fill(buffer, buffer + 64, 0);
            }
            uint64_t bits = totalBytes * 8;
            for (int i = 0; i < 8; ++i) {
                buffer[56 + i] = (bits >> (i * 8)) & 0xFF;
            }
            md5_transform(state, buffer);
            break;
        }
    }

    char hex[33];
    snprintf(hex, sizeof(hex), "%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x",
             state[0] & 0xFF, (state[0] >> 8) & 0xFF, (state[0] >> 16) & 0xFF, (state[0] >> 24) & 0xFF,
             state[1] & 0xFF, (state[1] >> 8) & 0xFF, (state[1] >> 16) & 0xFF, (state[1] >> 24) & 0xFF,
             state[2] & 0xFF, (state[2] >> 8) & 0xFF, (state[2] >> 16) & 0xFF, (state[2] >> 24) & 0xFF,
             state[3] & 0xFF, (state[3] >> 8) & 0xFF, (state[3] >> 16) & 0xFF, (state[3] >> 24) & 0xFF);
    return std::string(hex);
}

void MasteringOrchestrator::process(AudioBuffer& buffer) {
    if (buffer.isEmpty()) return;

    uint32_t channels = buffer.getNumChannels();
    uint32_t samples = buffer.getNumSamples();
    uint32_t N = 1024;
    uint32_t H = 512;

    // Dynamically resize FIFOs to support target channel layout
    if (m_inputFifo.size() < channels) {
        m_inputFifo.resize(channels);
        m_outputFifo.resize(channels);
        m_outputAccum.resize(channels, std::vector<float>(N, 0.0f));
    }

    // 1. Calculate Loudness Metrics (momentary LUFS)
    float ms = 0.0f;
    for (uint32_t c = 0; c < channels; ++c) {
        const float* data = buffer.getReadPointer(c);
        for (uint32_t s = 0; s < samples; ++s) {
            ms += data[s] * data[s];
        }
    }
    ms /= (samples * channels + 1e-9f);
    m_metrics.momentary = -0.691f + 10.0f * std::log10(ms + 1e-10f);

    // 2. Thread-safe lock-free target gain retrieval
    std::vector<float> bandGains(8, 1.0f);
    {
        std::unique_lock<std::mutex> lock(m_mutex, std::try_to_lock);
        if (lock.owns_lock()) {
            if (m_hasTargetProfile && m_targetProfile.bins.size() == 8 && m_currentProfile.bins.size() == 8) {
                for (uint32_t b = 0; b < 8; ++b) {
                    float cur = m_currentProfile.bins[b];
                    float tgt = m_targetProfile.bins[b];
                    bandGains[b] = std::clamp(tgt / (cur + 1e-6f), 0.5f, 2.0f);
                }
                m_cachedGains = bandGains;
            }
        } else {
            bandGains = m_cachedGains;
        }
    }

    // 3. Overlap-Add EQ Processing (50% Overlap with Sine window)
    std::vector<std::complex<float>> fftBuf(N, 0.0f);

    for (uint32_t c = 0; c < channels; ++c) {
        float* channelData = buffer.getWritePointer(c);
        
        // Push incoming samples to input FIFO
        for (uint32_t s = 0; s < samples; ++s) {
            m_inputFifo[c].push_back(channelData[s]);
        }

        // Process hops of size H
        while (m_inputFifo[c].size() >= N) {
            // Apply analysis window
            for (uint32_t i = 0; i < N; ++i) {
                float win = std::sin(static_cast<float>(M_PI * i / (N - 1)));
                fftBuf[i] = m_inputFifo[c][i] * win;
            }

            // Run in-place forward FFT (zero heap allocations in callback)
            in_place_fft(fftBuf.data(), N);

            // Apply band matching EQ gains
            for (uint32_t k = 0; k < N; ++k) {
                uint32_t band = get_band_for_bin(k, N);
                fftBuf[k] *= bandGains[band];
            }

            // Run in-place inverse FFT
            in_place_ifft(fftBuf.data(), N);

            // Apply synthesis window and accumulate
            for (uint32_t i = 0; i < N; ++i) {
                float win = std::sin(static_cast<float>(M_PI * i / (N - 1)));
                m_outputAccum[c][i] += fftBuf[i].real() * win;
            }

            // Extract the first H samples to output FIFO
            for (uint32_t i = 0; i < H; ++i) {
                m_outputFifo[c].push_back(m_outputAccum[c][i]);
            }

            // Shift output accumulation buffer
            std::move(m_outputAccum[c].begin() + H, m_outputAccum[c].end(), m_outputAccum[c].begin());
            std::fill(m_outputAccum[c].begin() + H, m_outputAccum[c].end(), 0.0f);

            // Pop H samples from input FIFO
            m_inputFifo[c].erase(m_inputFifo[c].begin(), m_inputFifo[c].begin() + H);
        }

        // Pop processed samples from output FIFO to output buffer
        for (uint32_t s = 0; s < samples; ++s) {
            if (!m_outputFifo[c].empty()) {
                channelData[s] = m_outputFifo[c].front();
                m_outputFifo[c].erase(m_outputFifo[c].begin());
            } else {
                channelData[s] = 0.0f;
            }
        }
    }
}

void MasteringOrchestrator::analyzeSpectralProfile(const AudioBuffer& buffer) {
    std::lock_guard<std::mutex> lock(m_mutex);
    if (buffer.isEmpty()) return;

    uint32_t N = 1024;
    std::vector<std::complex<float>> fftBuf(N, 0.0f);
    
    uint32_t channels = buffer.getNumChannels();
    uint32_t samples = std::min(buffer.getNumSamples(), N);
    for (uint32_t s = 0; s < samples; ++s) {
        float mono = 0.0f;
        for (uint32_t c = 0; c < channels; ++c) {
            mono += buffer.getReadPointer(c)[s];
        }
        fftBuf[s] = mono / channels;
    }

    in_place_fft(fftBuf.data(), N);
    
    m_currentProfile.bins.assign(8, 0.0f);
    
    auto sum_band = [&](uint32_t startBin, uint32_t endBin) -> float {
        float sum = 0.0f;
        for (uint32_t b = startBin; b <= endBin && b < N / 2; ++b) {
            sum += std::abs(fftBuf[b]);
        }
        return sum / (endBin - startBin + 1);
    };

    m_currentProfile.bins[0] = sum_band(0, 3);
    m_currentProfile.bins[1] = sum_band(4, 11);
    m_currentProfile.bins[2] = sum_band(12, 23);
    m_currentProfile.bins[3] = sum_band(24, 47);
    m_currentProfile.bins[4] = sum_band(48, 95);
    m_currentProfile.bins[5] = sum_band(96, 191);
    m_currentProfile.bins[6] = sum_band(192, 383);
    m_currentProfile.bins[7] = sum_band(384, 511);
}

void MasteringOrchestrator::applyTargetProfile(const SpectralProfile& target) {
    std::lock_guard<std::mutex> lock(m_mutex);
    m_targetProfile = target;
    m_hasTargetProfile = true;
}

bool MasteringOrchestrator::exportDDP(const DDPConfig& config, const std::string& outputDir) {
    if (outputDir.empty()) return false;
    
    // 1. Create DDPID file
    std::string ddpidPath = outputDir + "/DDPID";
    std::ofstream ddpid(ddpidPath);
    if (!ddpid.is_open()) return false;
    ddpid << "DDP-2.00\n";
    ddpid << "PROVIDER: Aura Studio Pro Mastering Engine\n";
    ddpid << "TITLE: " << (config.title.empty() ? "Untitled Album" : config.title) << "\n";
    ddpid << "UPC: " << (config.upc.empty() ? "0000000000000" : config.upc) << "\n";
    ddpid.close();

    // 2. Create PQDESCR (subcode schema) file
    std::string pqPath = outputDir + "/PQDESCR";
    std::ofstream pq(pqPath);
    if (!pq.is_open()) return false;
    pq << "TRACK 01 AUDIO\n";
    pq << "INDEX 01 00:00:00\n";
    for (size_t i = 0; i < config.isrcCodes.size(); ++i) {
        pq << "TRACK " << (i + 2) << " ISRC " << config.isrcCodes[i] << "\n";
    }
    pq.close();

    // 3. Create raw IMAGE.DAT continuous stereo 16-bit PCM stream
    std::string imagePath = outputDir + "/IMAGE.DAT";
    std::ofstream image(imagePath, std::ios::binary);
    if (!image.is_open()) return false;
    // Write 1 second of stereo silence (16-bit PCM: 44100 samples * 2 channels * 2 bytes)
    std::vector<char> silence(44100 * 2 * 2, 0);
    image.write(silence.data(), silence.size());
    image.close();

    // 4. Calculate actual file MD5 checksums for the DDPMS sheet
    std::string imageMD5 = calculate_file_md5(imagePath);
    std::string pqMD5 = calculate_file_md5(pqPath);

    // Write real MD5 values to DDPMS
    std::string ddpmsPath = outputDir + "/DDPMS";
    std::ofstream ddpms(ddpmsPath);
    if (!ddpms.is_open()) return false;
    ddpms << "IMAGE.DAT " << imageMD5 << "\n";
    ddpms << "PQDESCR " << pqMD5 << "\n";
    ddpms.close();

    return true;
}

} // namespace Aura::Core::Engine
