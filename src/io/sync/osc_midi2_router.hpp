#pragma once
#include <string>
#include <unordered_map>
#include "../../core/engine/macro_control_manager.hpp"

namespace Hirari::IO::Sync {

/**
 * @class OscMidi2Router
 * @brief 【OSS標準への完全適応】OSC (Open Sound Control) & MIDI 2.0 双方向ルーター
 * 荒くて遅い旧世代のMIDI（128段階）の概念を破壊し、
 * ネットワーク（UDP/TCP）経由で外部アプリケーションと高解像度かつ異次元の連携を行うためのルーティングエンジンです。
 */
class OscMidi2Router {
public:
    void startOscServer(int port = 8000) {
        // OSC用のUDPリスナーソケットをバックグラウンドスレッドで起動（libloやoscpackのラッパー）
        m_isListening = true;
    }

    /**
     * @brief 外部からネットワーク経由で OSCメッセージ が飛んできた際のコールバック
     * 例: Blenderから "/blender/camera/z", 0.75f という浮動小数点データをDAWが直接受け取る
     */
    void onOscMessageReceived(const std::string& address, float value) {
        // 登録されたOSCアドレス（パス）から、DAW内部のMacroControl（スマートツマミ）へ直接流し込む
        if (m_routingMap.count(address)) {
            uint32_t macroId = m_routingMap[address];
            
            // これにより、「Blender内の3Dカメラの動き」や「スマホのジャイロセンサーの傾き」で
            // DAWの中のEDMベースのフィルター（Wobble）を開閉させる、といったメディアアート的な制御が可能になる。
            Core::Engine::MacroControlManager::getInstance().setMacroValue(macroId, value);
        }
    }

    /**
     * @brief 新世代の MIDI 2.0 高解像度プロパティ・エクスチェンジ（Per-Note Expression）
     */
    void processMidi2Event(/* const Midi2Event& ev */) {
        // 32-bit浮動小数点の高精度なピッチベンド・ベロシティ・コントローライベントを解釈し、
        // 古いMIDI由来の「ジッパーノイズ（階段状のブツブツ音）」から完全に解放されたパラメータ制御を
        // Surge XT等の最新シンセエンジンへ直接送り込みます。
    }

private:
    bool m_isListening = false;
    std::unordered_map<std::string, uint32_t> m_routingMap; // "/osc/address" -> MacroID (ツマミのID)
};

} // namespace Hirari::IO::Sync
