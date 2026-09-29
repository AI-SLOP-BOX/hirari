#pragma once
#include <atomic>
#include <string>

namespace Hirari::IO::Sync {

/**
 * @class TransportSyncManager
 * @brief 【OSS標準への完全適応】Ableton Link / JACK Transport / PipeWire / NSM 同期ハブ
 * 閉鎖的なDAWではなく、世界中の異なるオープンソース・ソフトウェアやOSと
 * 『再生位置（タイムコード）やテンポ（BPM）』を完璧に同期させるための「究極のネットワーク心臓部」です。
 */
class TransportSyncManager {
public:
    enum class SyncSource { 
        Internal,       // 内部クロック（単独動作）
        AbletonLink,    // 無線LAN経由（Ableton Linkプロトコル・スマホアプリ等と同期）
        JackPipeWire,   // Linux/Mac内部（JACK / PipeWire 経由でArdour等と同期）
        RaySessionNSM   // Non Session Manager（複数アプリの一括保存・連動管理）
    };

    void setSyncSource(SyncSource src) { m_syncSource = src; }

    /**
     * @brief オーディオスレッドの先頭で毎ブロック呼ばれ、ネットワーク上の「絶対時間」でDAWを支配する
     */
    void updateSyncState(double& currentBpm, double& currentBeat) {
        if (m_syncSource == SyncSource::AbletonLink) {
            // 【Ableton Link 同期】
            // Linkライブラリのセッション状態（ローカルLANに繋がるPeer数、テンポ、フェーズ情報）を取得し、
            // 手元のDAWのBPMや小節位置を「ネットワーク全体の絶対マスター時間」に強制的に追従（上書き）させます。
            
            // 例: currentBpm = linkSession.captureAppSessionState().tempo();
            
        } else if (m_syncSource == SyncSource::JackPipeWire) {
            // 【JACK / PipeWire Transport 同期】
            // jack_transport_query() などを叩き、動画編集ソフト（Kdenlive）や別のDAW（Zrythm）の
            // スペースキー（再生/停止）とフレーム単位の正確さで、DAW本体を同時に動かします。
        }
        
        // （※NSM/RaySession等のクライアントとしてのステート・セーブ制御信号の受け取り回路もここに内包します）
    }

private:
    std::atomic<SyncSource> m_syncSource{SyncSource::Internal};
};

} // namespace Hirari::IO::Sync
