#pragma once
#include <string>
#include <vector>
#include <cstdlib>
#include <filesystem>
#include <cctype>

namespace Aura::IO::Media {

/**
 * @class CliMediaProcessor
 * @brief 【OSS標準への完全適応】FFmpeg / SoX メディアバッチエンジン
 * DAW自体の「書き出し機能」とは完全に切り離された、OS不問のマルチメディア・コマンドライン心臓部です。
 * 映像の結合（Kdenlive等との連携）や、数百にのぼる音声ファイルのサンプルレート一括変換などを
 * オープンソース界のメディア王である「FFmpeg」と「SoX」に直接CLIプロセスを通してブン投げ、
 * 数十時間の単純作業を1秒で終わらせるバッチ処理インフラです。
 */
class CliMediaProcessor {
public:
    /**
     * @brief 完成したDAWのWAV音源を、FFmpegを使って元の動画ファイル（MP4等）にマージ（結合）する
     */
    static bool muxAudioToVideo(const std::string& audioPath, const std::string& videoPath, const std::string& outputPath) {
        const std::string video = quoteShellArgument(videoPath);
        const std::string audio = quoteShellArgument(audioPath);
        const std::string output = quoteShellArgument(outputPath);
        if (video.empty() || audio.empty() || output.empty()) return false;

        // Keep the legacy CLI integration, but never interpolate raw user
        // paths into a shell command.  Quoting is platform-specific and the
        // Windows path policy fails closed for cmd.exe metacharacters.
        const std::string cmd = "ffmpeg -nostdin -i " + video + " -i " + audio +
                                " -c:v copy -c:a aac -b:a 320k " + output +
                                " -y -v error";
        const int result = std::system(cmd.c_str());
        return (result == 0); 
    }

    /**
     * @brief SoX (Sound eXchange) を用いた、数百個の録音テイクの一掃バッチ処理・トリミング
     */
    static bool batchTrimSilenceUsingSoX(const std::vector<std::string>& files) {
        bool success = true;
        for (const auto& file : files) {
            const std::filesystem::path source(file);
            std::error_code error;
            if (!std::filesystem::is_regular_file(source, error) || error) {
                success = false;
                continue;
            }
            const auto parent = source.parent_path();
            const auto stem = source.stem().string();
            const auto extension = source.extension().string();
            const auto destination = parent / (stem + ".trimmed" + extension);
            const std::string input = quoteShellArgument(source.string());
            const std::string output = quoteShellArgument(destination.string());
            if (input.empty() || output.empty()) {
                success = false;
                continue;
            }

            const std::string cmd = "sox -V0 " + input + " " + output +
                                    " silence 1 0.1 1% reverse silence 1 0.1 1% reverse";
            if (std::system(cmd.c_str()) != 0) success = false;
        }
        return success;
    }

private:
    static std::string quoteShellArgument(const std::string& value) {
        if (value.empty() || value.find('\0') != std::string::npos) return {};
#if defined(_WIN32)
        // `std::system` uses cmd.exe on Windows. Reject its control and
        // expansion characters rather than attempting a partial escaping
        // scheme that could vary with shell settings.
        for (const unsigned char c : value) {
            if (c < 0x20u || c == '"' || c == '&' || c == '|' || c == '<' ||
                c == '>' || c == '^' || c == '%' || c == '!') {
                return {};
            }
        }
        return "\"" + value + "\"";
#else
        // POSIX shells treat everything inside single quotes literally;
        // represent an embedded quote as three adjacent quoted segments.
        std::string quoted;
        quoted.reserve(value.size() + 2);
        quoted.push_back('\'');
        for (const char c : value) {
            if (c == '\'') quoted += "'\\''";
            else quoted.push_back(c);
        }
        quoted.push_back('\'');
        return quoted;
#endif
    }
};

} // namespace Aura::IO::Media
