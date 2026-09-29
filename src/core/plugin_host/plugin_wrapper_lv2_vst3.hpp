#pragma once
#include <string>
#include <memory>
#include "../../dsp/iprocessor.hpp"
#include "../plugins/plugin_host.hpp"

namespace Hirari::Core::PluginHost {

/**
 * @class ExternalPluginWrapper
 * @brief Compatibility wrapper that routes legacy callers to the real
 * process-isolated external-plugin processor.
 *
 * This type used to be a silent structural placeholder.  Keeping that class
 * alive is useful for old graph construction code, but it must not create a
 * fake successful plugin.  All supported formats now flow through the same
 * lifecycle and worker transport used by the production plugin host.
 */
class ExternalPluginWrapper : public DSP::IProcessor {
public:
    enum class Format { VST3, LV2, AudioUnit, CLAP };

    ExternalPluginWrapper(Format fmt, const std::string& pluginPath)
        : m_path(pluginPath) {
        const auto description = ::Hirari::Core::Plugins::PluginDescription{
            pluginPath.empty() ? std::string{} : pluginPath,
            "Unknown",
            toPluginFormat(fmt),
            pluginPath};
        m_processor = std::make_unique<::Hirari::Core::Plugins::ExternalPluginProcessor>(description);
        if (!m_processor->load()) m_error = m_processor->lastError();
    }

    // ネイティブDAWエンジンから外部プラグインへのサンプリングレート／バッファ情報セット
    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        if (m_processor) m_processor->prepareToPlay(sr, bs);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const DSP::ProcessContext& context) noexcept override {
        if (m_processor) {
            m_processor->process(buffer, midi, context);
            return;
        }
        buffer.clear();
    }

    void reset() noexcept override { if (m_processor) m_processor->reset(); }

    uint32_t getLatencySamples() const noexcept override {
        // 【PDCの根幹】PDC（プラグイン・ディレイ・コンペンセーション）のための遅延報告。
        // ZrythmやArdour等で最も計算が複雑になるグラフ・トポロジー遅延計算の核となる数値。
        // VST3プラグインが宣言する内部処理遅延をDAW本体に吸い上げます。
        return m_processor ? m_processor->getLatencySamples() : m_reportedLatency;
    }

    bool supportsProcessing() const noexcept {
        // Loading the wrapper only prepares the sandbox object. It is not a
        // usable processor until the audio configuration has been applied and
        // the worker is operational.
        return m_processor && m_processor->isOperational();
    }
    bool restoreStateChecked(const std::vector<uint8_t>& state) override {
        return m_processor && m_processor->restoreStateChecked(state);
    }
    std::vector<uint8_t> getState() const override {
        return m_processor ? m_processor->getState() : std::vector<uint8_t>{};
    }
    const std::string& errorMessage() const noexcept { return m_error; }

private:
    static ::Hirari::Core::Plugins::PluginFormat toPluginFormat(Format format) noexcept {
        switch (format) {
            case Format::VST3: return ::Hirari::Core::Plugins::PluginFormat::VST3;
            case Format::AudioUnit: return ::Hirari::Core::Plugins::PluginFormat::AU;
            case Format::CLAP: return ::Hirari::Core::Plugins::PluginFormat::CLAP;
            case Format::LV2: return ::Hirari::Core::Plugins::PluginFormat::Internal;
        }
        return ::Hirari::Core::Plugins::PluginFormat::Internal;
    }

    std::string m_path;
    std::unique_ptr<::Hirari::Core::Plugins::ExternalPluginProcessor> m_processor;
    std::string m_error;
    uint32_t m_reportedLatency = 0; // プラグインから通知されたレイテンシー
};

} // namespace Hirari::Core::PluginHost
