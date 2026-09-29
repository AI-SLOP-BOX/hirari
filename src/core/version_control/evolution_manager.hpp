#pragma once
#include <string>
#include <vector>
#include <map>
#include <chrono>
#include <nlohmann/json.hpp>

namespace Hirari::Core::VersionControl {

/**
 * @class ProjectEvolutionManager
 * @brief 【OSS独自の最強プラットフォーム化：音声データ特化型Gitエンジン】
 * 
 * 従来のDAWが「巨大なバイナリファイル」を扱うのに対し、Hirariはプロジェクトの状態を
 * 高密度なJSONとデルタ（差分）で管理します。これにより、複数人での同時編集や
 * 「昨日、エフェクトをかける前の音に戻したい」といった変更履歴を完璧にトレースします。
 */
class ProjectEvolutionManager {
public:
    struct Commit {
        std::string id;
        std::string author;
        std::string message;
        uint64_t timestamp;
        nlohmann::json delta; // State difference
    };

    static ProjectEvolutionManager& getInstance() { static ProjectEvolutionManager i; return i; }

    /**
     * @brief SNAPSHOT: 現在のエンジンの全状態をコミット
     */
    void commit(const std::string& message, const nlohmann::json& currentState) {
        Commit c;
        c.timestamp = std::chrono::system_clock::to_time_t(std::chrono::system_clock::now());
        c.message = message;
        c.id = generateHash(c.timestamp);
        
        // Calculate Delta from last state
        if (!m_history.empty()) {
            c.delta = nlohmann::json::diff(m_lastFullState, currentState);
        } else {
            c.delta = currentState; // Initial state
        }
        
        m_history.push_back(c);
        m_lastFullState = currentState;
        std::cout << "[Evolution] New Commit: " << c.id << " (" << message << ")\n";
    }

    /**
     * @brief CHECKOUT: 特定の過去のバージョンへ瞬時に復帰
     */
    nlohmann::json checkout(const std::string& commitId) {
        nlohmann::json state = m_history.front().delta; // Start from base
        for (const auto& c : m_history) {
            state = state.patch(c.delta);
            if (c.id == commitId) break;
        }
        return state;
    }

    /**
     * @brief GET_HISTORY: UI表示用の履歴リストを取得
     */
    std::vector<Commit> getHistory() const { return m_history; }

private:
    std::string generateHash(uint64_t t) {
        return "rev_" + std::to_string(t).substr(5);
    }

    std::vector<Commit> m_history;
    nlohmann::json m_lastFullState;
};

} // namespace Hirari::Core::VersionControl
