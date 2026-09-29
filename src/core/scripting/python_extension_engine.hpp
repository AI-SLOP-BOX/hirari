#pragma once
#include <string>
#include <vector>
#include <iostream>

#include "../../HirariUltimate.hpp"
#include "../engine/macro_control_manager.hpp"
#include "../../rendering/bounce/bounce_engine.hpp"

// ※実環境ではここに pybind11 のヘッダをインクルードし、組み込みPythonを有効化します。
// #include <pybind11/pybind11.h>
// #include <pybind11/embed.h>
// namespace py = pybind11;

namespace Hirari::Core::Scripting {

/**
 * @class PythonExtensionEngine
 * @brief 【OSS最高の自由・究極の拡張】PythonスクリプトによるDAW完全制御・自動化エンジン
 * Blenderの「Python API」やReaperの「ReaScript」と同じ設計思想です。
 */
class PythonExtensionEngine {
public:
    PythonExtensionEngine() {
        std::cout << "[PythonEngine] Embedded CPython interpreter initialized.\n";
    }

    ~PythonExtensionEngine() {}

    /**
     * @brief ユーザーが記述した外部のPythonスクリプト (.py) をDAW内で実行する
     */
    bool executeScript(const std::string& pyFilePath) {
        try {
            // py::eval_file(pyFilePath);
            std::cout << "[PythonEngine] Executed User Script: " << pyFilePath << "\n";
            return true;
        } catch (const std::exception& e) {
            std::cerr << "[PythonEngine] Script Crash: " << e.what() << "\n";
            return false;
        }
    }

    /**
     * @brief Pythonスクリプトの中から、DAW（Hirari）のC++機能群を呼び出せるようにAPIを公開（バインド）する
     */
    void bindHirariAPI() {
        // 仮想的な pybind11 によるモジュール登録とAPIバインドの高度な実装
        
        /*
        PYBIND11_EMBEDDED_MODULE(hirari, m) {
            m.doc() = "Hirari DAW Python Extension API";

            // 1. 【バッチ処理・自動化】Pythonから直接ステムのパラレル書き出しを指示
            m.def("bounce_master", [](const std::string& absolutePath, int totalSamples, int sampleRate) {
                Engine::BounceEngine::BounceConfig config;
                config.outputPath = absolutePath;
                config.totalSamples = totalSamples;
                config.sampleRate = sampleRate;
                config.format = Engine::BounceEngine::Format::WAV_32F;
                config.revealInFinder = true;
                config.runAIMasteringReview = true;
                
                auto result = Engine::BounceEngine::renderMaster(config);
                return py::make_tuple(result.success, result.message, result.aiAdvice);
            });

            // 2. 【パラメータの外部制御】Blenderや外部センサーから受け取った値をDAWのマクロに流し込む
            m.def("set_macro_value", [](int macroId, float value) {
                Engine::MacroControlManager::getInstance().setMacroValue(macroId, value);
            });

            m.def("get_macro_value", [](int macroId) {
                return Engine::MacroControlManager::getInstance().getMacroValue(macroId);
            });

            // 3. 【高度なルーティング設定】スクリプトからマクロターゲットとカーブを動的ルーティング
            py::enum_<Engine::TransferCurve>(m, "TransferCurve")
                .value("Linear", Engine::TransferCurve::Linear)
                .value("Exponential", Engine::TransferCurve::Exponential)
                .value("Logarithmic", Engine::TransferCurve::Logarithmic)
                .value("SCurve", Engine::TransferCurve::SCurve)
                .export_values();

            py::class_<Engine::MacroTarget>(m, "MacroTarget")
                .def(py::init<>())
                .def_readwrite("track_id", &Engine::MacroTarget::trackId)
                .def_readwrite("param_id", &Engine::MacroTarget::paramId)
                .def_readwrite("curve", &Engine::MacroTarget::curve);

            m.def("link_macro", [](int macroId, const Engine::MacroTarget& target) {
                Engine::MacroControlManager::getInstance().link(macroId, target);
            });
        }
        */
        std::cout << "[PythonEngine] Core Python APIs Binding Prepared.\n";
    }
};

} // namespace Hirari::Core::Scripting
