#pragma once

#include <vector>
#include <string>
#include <iostream>
#include <chrono>
#include <functional>
#include <memory>
#include <atomic>
#include <cstdlib>
#include <algorithm>
#include <limits>
#include <filesystem>
#if defined(__APPLE__)
#include <spawn.h>
extern char** environ;
#endif
#include "../../AuraUltimate.hpp"
#include "../../scae/AuraAISuite.hpp"
#include "../../core/io/ffmpeg_engine.hpp"
#include "../../io/persistence/wav_writer.hpp"

namespace Aura::Core::Engine {

/**
 * @class BounceEngine
 * @brief Professional Offline Rendering & Stem Export System.
 * 【超絶肉付け】単なるファイル書き出しから「書き出しワークフロー全体」をカバーするシステムに昇華。
 * 1. WAV/RF64/WAVE64対応の基盤（未接続codecは明示的に拒否）
 * 2. プログレスバーUIのためのリアルタイム進捗コールバック機能
 * 3. AIによる書き出し後のマスタリング自動レビュー
 * 4. Finder/Explorerでの自動ファイル表示 (Logic Proのあの便利な機能)
 */
class BounceEngine {
public:
    enum class Format { WAV_16, WAV_24, WAV_32F, WAVE64_32F, MP3, FLAC };

    struct BounceConfig {
        std::string outputPath;
        uint64_t totalSamples;
        uint32_t sampleRate;
        Format format = Format::WAV_32F;
        bool revealInFinder = true;
        bool runAIMasteringReview = true;
        const std::atomic<bool>* cancellation = nullptr;
    };

    struct BounceResult {
        bool success;
        double elapsedSeconds;
        std::string aiAdvice;
        std::string message;
    };

    /**
     * @brief MASTER BOUNCE: タイムライン全体をオフライン（非リアルタイム）で最速レンダリングします。
     * @param onProgress UIスレッド（プログレスバー）へ進捗(0.0 - 1.0)を通知するコールバック
     */
    static BounceResult renderMaster(const BounceConfig& config, std::function<void(float)> onProgress = nullptr) {
        auto startTime = std::chrono::high_resolution_clock::now();
        if (config.outputPath.empty() || config.totalSamples == 0 || config.sampleRate == 0 ||
            config.sampleRate > 384000) {
            return {false, 0.0, "", "Invalid render configuration."};
        }
        // Encode through a bounded float-WAV staging file, then publish only
        // the completed codec output.  This keeps the audio graph independent
        // from the external encoder and prevents partial MP3/FLAC files from
        // appearing at the requested destination.
        if (config.format == Format::MP3 || config.format == Format::FLAC) {
            static std::atomic<uint64_t> sequence{0};
            const auto id = sequence.fetch_add(1, std::memory_order_relaxed);
            const std::filesystem::path output(config.outputPath);
            const auto stagingWav = output.string() + ".tmp-aura-encode-source-" + std::to_string(id) + ".wav";
            const auto stagingEncoded = output.string() + ".tmp-aura-encoded-" + std::to_string(id);
            BounceConfig wavConfig = config;
            wavConfig.outputPath = stagingWav;
            wavConfig.format = Format::WAV_32F;
            wavConfig.revealInFinder = false;
            wavConfig.runAIMasteringReview = false;
            auto rendered = renderMaster(wavConfig, onProgress);
            std::error_code cleanup;
            if (!rendered.success) {
                std::filesystem::remove(stagingWav, cleanup);
                return rendered;
            }
            const char* codec = config.format == Format::MP3 ? "libmp3lame" : "flac";
            if (!::Aura::Core::IO::FFmpegEngine::getInstance().exportToFormat(
                    stagingWav, stagingEncoded, codec, config.cancellation)) {
                std::filesystem::remove(stagingWav, cleanup);
                std::filesystem::remove(stagingEncoded, cleanup);
                return {false, rendered.elapsedSeconds, "", "External audio encoder failed."};
            }
            std::filesystem::remove(stagingWav, cleanup);
            std::filesystem::rename(stagingEncoded, output, cleanup);
            if (cleanup) {
                std::filesystem::remove(stagingEncoded, cleanup);
                return {false, rendered.elapsedSeconds, "", "Encoded output publication failed."};
            }
            if (onProgress) onProgress(1.0f);
            rendered.message = "Render and codec export completed successfully.";
            return rendered;
        }
        // ★注意：この機能はオーディオスレッドとは別の「バックグラウンド・ワーカー・スレッド」で実行されます。
        auto& engine = ::Aura::AuraEngine::getInstance();

        // The normal WAV/WAVE64 delivery path is streamed.  Keeping an entire
        // song in two vectors made a long bounce scale linearly with duration
        // and could exhaust the process before the atomic writer ran.  The
        // bounded analysis preview below is intentionally independent from
        // the published audio payload.
        if (config.format == Format::WAV_16 || config.format == Format::WAV_24 ||
            config.format == Format::WAV_32F || config.format == Format::WAVE64_32F) {
            constexpr uint32_t kRenderBlockSize = 1024;
            constexpr size_t kMaxAnalysisSamples = 1u << 20;
            std::vector<float> blockL(kRenderBlockSize, 0.0f);
            std::vector<float> blockR(kRenderBlockSize, 0.0f);
            std::vector<float> analysisL;
            std::vector<float> analysisR;
            const uint64_t analysisStride = std::max<uint64_t>(
                1, (config.totalSamples + kMaxAnalysisSamples - 1) / kMaxAnalysisSamples);
            if (config.runAIMasteringReview) {
                analysisL.reserve(static_cast<size_t>(std::min<uint64_t>(
                    kMaxAnalysisSamples, config.totalSamples)));
                analysisR.reserve(analysisL.capacity());
            }

            std::unique_ptr<::Aura::IO::Persistence::WavWriter::Pcm16StreamWriter> pcm16;
            std::unique_ptr<::Aura::IO::Persistence::WavWriter::Pcm24StreamWriter> pcm24;
            std::unique_ptr<::Aura::IO::Persistence::WavWriter::Float32StreamWriter> float32;
            std::unique_ptr<::Aura::IO::Persistence::WavWriter::Wave64FloatStreamWriter> wave64;
            if (config.format == Format::WAV_16) {
                pcm16 = std::make_unique<::Aura::IO::Persistence::WavWriter::Pcm16StreamWriter>(
                    config.outputPath, config.totalSamples, config.sampleRate, 2);
            } else if (config.format == Format::WAV_24) {
                pcm24 = std::make_unique<::Aura::IO::Persistence::WavWriter::Pcm24StreamWriter>(
                    config.outputPath, config.totalSamples, config.sampleRate, false, 2);
            } else {
                if (config.format == Format::WAVE64_32F) {
                    wave64 = std::make_unique<::Aura::IO::Persistence::WavWriter::Wave64FloatStreamWriter>(
                        config.outputPath, config.totalSamples, config.sampleRate, 2);
                } else {
                float32 = std::make_unique<::Aura::IO::Persistence::WavWriter::Float32StreamWriter>(
                    config.outputPath, config.sampleRate, 2);
                }
            }
            const auto writerError = [&]() -> std::string {
                if (pcm16) return pcm16->error();
                if (pcm24) return pcm24->error();
                if (wave64) return wave64->error();
                return float32->error();
            };
            const auto writerOpen = [&]() {
                if (pcm16) return pcm16->isOpen();
                if (pcm24) return pcm24->isOpen();
                if (wave64) return wave64->isOpen();
                return float32->isOpen();
            };
            if (!writerOpen()) return {false, 0.0, "", writerError()};

            uint64_t rendered = 0;
            uint64_t nextAnalysisSample = 0;
            while (rendered < config.totalSamples) {
                if (config.cancellation && config.cancellation->load(std::memory_order_acquire)) {
                    return {false, 0.0, "", "Render cancelled."};
                }
                const uint32_t count = static_cast<uint32_t>(std::min<uint64_t>(
                    kRenderBlockSize, config.totalSamples - rendered));
                engine.process(blockL.data(), blockR.data(), count);
                bool written = false;
                if (pcm16) written = pcm16->writeFrames(blockL.data(), blockR.data(), count);
                else if (pcm24) written = pcm24->writeFrames(blockL.data(), blockR.data(), count);
                else {
                    const float* channels[2] = {blockL.data(), blockR.data()};
                    written = wave64 ? wave64->writeFrames(channels, count)
                                     : float32->writeFrames(channels, count);
                }
                if (!written) return {false, 0.0, "", writerError()};
                if (config.runAIMasteringReview) {
                    for (uint32_t i = 0; i < count && analysisL.size() < kMaxAnalysisSamples; ++i) {
                        const uint64_t absolute = rendered + i;
                        if (absolute >= nextAnalysisSample) {
                            analysisL.push_back(blockL[i]);
                            analysisR.push_back(blockR[i]);
                            nextAnalysisSample = absolute > std::numeric_limits<uint64_t>::max() - analysisStride
                                ? std::numeric_limits<uint64_t>::max()
                                : absolute + analysisStride;
                        }
                    }
                }
                rendered += count;
                if (onProgress && (rendered / kRenderBlockSize) % 50 == 0)
                    onProgress(static_cast<float>(rendered) / static_cast<float>(config.totalSamples));
            }
            bool finished = false;
            if (pcm16) finished = pcm16->finish();
            else if (pcm24) finished = pcm24->finish();
            else if (wave64) finished = wave64->finish();
            else finished = float32->finish();
            if (!finished) return {false, 0.0, "", writerError()};

            std::string advice;
            if (config.runAIMasteringReview && !analysisL.empty()) {
                ::Aura::SCAE::Intelligence::SCAEAdvisor advisor(config.sampleRate);
                advice = advisor.analyzeMaster(analysisL.data(), analysisR.data(), analysisL.size());
            }
            if (onProgress) onProgress(1.0f);
            const auto endTime = std::chrono::high_resolution_clock::now();
            return {true, std::chrono::duration<double>(endTime - startTime).count(),
                    advice, "Render completed successfully."};
        }
        
        // Remaining compatibility paths are retained below for legacy
        // callers; ordinary WAV/RF64/WAVE64 master output is streaming above.
        std::vector<float> exportL(config.totalSamples, 0.0f);
        std::vector<float> exportR(config.totalSamples, 0.0f);
        
        const uint32_t blockSize = 1024; // オフラインレンダリング用の大きなブロックサイズ
        
        for (uint64_t pos = 0; pos < config.totalSamples; pos += blockSize) {
            if (config.cancellation && config.cancellation->load(std::memory_order_acquire)) {
                return {false, 0.0, "", "Render cancelled."};
            }
            uint32_t currentBlock = static_cast<uint32_t>(std::min(static_cast<uint64_t>(blockSize), config.totalSamples - pos));
            
            // エンジンを非リアルタイムモードで駆動（プラグインのハイレゾリューション・モード等をトリガー）
            engine.process(exportL.data() + pos, exportR.data() + pos, currentBlock);
            
            // UIへの進捗報告 (50ブロックに1回程度の頻度でUIを更新し、UIスレッドの詰まりを防ぐ)
            if (onProgress && (pos / blockSize) % 50 == 0) {
                float percent = static_cast<float>(pos) / config.totalSamples;
                onProgress(percent);
            }
        }
        
        if (onProgress) onProgress(1.0f); // 100%
        
        // フォーマットに応じた書き出し (ここではWAVを代表として処理)
        const bool writeSuccess = config.format == Format::WAVE64_32F
            ? ::Aura::IO::Persistence::WavWriter::writeWave64(
                config.outputPath, exportL.data(), exportR.data(), config.totalSamples, config.sampleRate)
            : ::Aura::IO::Persistence::WavWriter::write(
                config.outputPath, exportL.data(), exportR.data(), config.totalSamples, config.sampleRate);
        if (!writeSuccess) return {false, 0.0, "", "Disk write failed."};

        // 【AI マスタリング・レビュー】
        std::string advice = "";
        if (config.runAIMasteringReview) {
            ::Aura::SCAE::Intelligence::SCAEAdvisor advisor(config.sampleRate);
            advice = advisor.analyzeMaster(exportL.data(), exportR.data(), config.totalSamples);
        }

        auto endTime = std::chrono::high_resolution_clock::now();
        std::chrono::duration<double> elapsed = endTime - startTime;

        // 【自動ファイル展開】 macOS限定で、書き出したファイルをFinderでハイライト表示する
        if (config.revealInFinder) {
            #ifdef __APPLE__
            // Never interpolate a project path into a shell command. Paths
            // can contain quotes, substitutions, or shell metacharacters.
            // Pass the path as an argv element directly to LaunchServices.
            pid_t child = 0;
            char openArg[] = "open";
            char revealArg[] = "-R";
            char* argv[] = {openArg, revealArg,
                            const_cast<char*>(config.outputPath.c_str()), nullptr};
            (void)::posix_spawnp(&child, "open", nullptr, nullptr, argv, ::environ);
            #endif
        }

        return {true, elapsed.count(), advice, "Render completed successfully in " + std::to_string(elapsed.count()) + "s"};
    }
};

} // namespace Aura::Core::Engine
