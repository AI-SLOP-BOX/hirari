#pragma once
#include <string>
#include <vector>
// #include <sqlite3.h> // 実際の環境ではSQLiteランタイムをリンクします

namespace Hirari::Core::Database {

/**
 * @class SqliteAssetDatabase
 * @brief 【OSS標準・究極の進化】SQLiteによる超高速メタデータ（Apple Loops代替）検索エンジン
 * 数万個に及ぶWAV・AIFFの生音素材や、先日落とした「VSCO2のオーケストラ音源」、
 * さらには最強シンセ Surge XT の2800種のプリセットを、世界標準の軽量リレーショナルデータベース『SQLite』で一元管理します。
 * 
 * Logic Proの検索窓のように、「Bass」「Tech House」「120BPM」といったタグを入力するだけで、
 * 0.001秒（ミリ秒単位）でディスク上のフルパスとメタデータを「SELECT文」で返し、
 * メディアブラウザUIにリストとして表示させるためのデータ基盤です。
 */
class SqliteAssetDatabase {
public:
    struct AudioAsset {
        uint64_t id;
        std::string filepath;
        std::string name;
        std::string category; // 例: "Drums", "Synth"
        float bpm;            // -1.0の場合はループ以外のワンショット扱い
        int key;              // 基準音程。C=0, C#=1... -1の場合はドローン等の無音階
    };

    SqliteAssetDatabase() {
        // [SQLite初期化]
        // sqlite3_open("hirari_assets.db", &db);
        
        // 【テーブル定義例】
        // "CREATE TABLE IF NOT EXISTS loop_assets ("
        // "  id INTEGER PRIMARY KEY,"
        // "  path TEXT UNIQUE,"
        // "  name TEXT,"
        // "  category TEXT,"
        // "  bpm REAL,"
        // "  musical_key INTEGER"
        // ")";
        // 頻繁に検索されるBPMやCategory列には CREATE INDEX を貼って爆速化します。
    }

    /**
     * @brief ユーザーがUIの検索窓に入力したキーワードと、現在の曲のテンポ（BPM）に合う音源を一瞬で取得
     */
    std::vector<AudioAsset> searchAssets(const std::string& queryTags, float targetBpmMatch = -1.0f) {
        std::vector<AudioAsset> results;
        // [SQLiteクエリ発行（インジェクション対策としてPreparedStatementを使用想定）]
        
        // 例: "SELECT * FROM loop_assets WHERE (category LIKE '%Drum%' OR name LIKE '%Clap%')"
        //     "AND (bpm BETWEEN 115 AND 125)"
        
        // ヒットした行（row）をパースしてAudioBufferへ即座に読み込む準備をします。（モック）
        return results;
    }

    /**
     * @brief （コマンドライン用）フォルダ内の数万のWAVファイルを一括で解析してデータベースにブチ込む
     */
    void indexDirectory(const std::string& rootPath) {
        // フォルダを再帰検索し、ファイル名のヘッダ付近にある「テンポ」や「キー」のテキスト
        // （例： 120_Em_SynthBass.wav）を正規表現で抽出し、
        // まとめて SQLite に "INSERT INTO" の一括トランザクションを発行（超高速インデクシング）。
    }
};

} // namespace Hirari::Core::Database
