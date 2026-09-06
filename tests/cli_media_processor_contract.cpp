#include "../src/io/media/cli_media_processor.hpp"

#include <algorithm>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <string>

#if !defined(_WIN32)
#include <cstdlib>
#endif

int main() {
    using Aura::IO::Media::CliMediaProcessor;

    if (CliMediaProcessor::muxAudioToVideo("", "video.mp4", "out.mp4")) return 1;
    if (CliMediaProcessor::batchTrimSilenceUsingSoX({"/definitely/missing-audio.wav"})) return 2;

#if !defined(_WIN32)
    // Route the command to a local recorder and use shell metacharacters in a
    // path. If quoting regresses, the injected `touch` creates the marker.
    const auto nonce = std::chrono::steady_clock::now().time_since_epoch().count();
    const auto root = std::filesystem::temp_directory_path() /
        ("aura-cli-media-contract-" + std::to_string(nonce));
    std::filesystem::create_directories(root);
    const auto recorder = root / "ffmpeg";
    const auto argvFile = root / "argv.txt";
    const auto marker = root / "injected-marker";
    {
        std::ofstream script(recorder);
        script << "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$AURA_CLI_ARGV\"\nexit 0\n";
    }
    std::filesystem::permissions(
        recorder,
        std::filesystem::perms::owner_exec | std::filesystem::perms::owner_read |
            std::filesystem::perms::owner_write,
        std::filesystem::perm_options::add);

    const char* oldPath = std::getenv("PATH");
    const std::string oldPathValue = oldPath ? oldPath : "";
    const std::string malicious = (root / ("voice'; touch " + marker.string() + ";.wav")).string();
    ::setenv("PATH", root.c_str(), 1);
    ::setenv("AURA_CLI_ARGV", argvFile.c_str(), 1);
    const bool ran = CliMediaProcessor::muxAudioToVideo(
        malicious, "video file.mp4", (root / "render file.mp4").string());
    if (oldPath) ::setenv("PATH", oldPathValue.c_str(), 1);
    else ::unsetenv("PATH");
    ::unsetenv("AURA_CLI_ARGV");

    bool exactPathObserved = false;
    if (ran) {
        std::ifstream args(argvFile);
        std::string line;
        while (std::getline(args, line)) {
            if (line == malicious) exactPathObserved = true;
        }
    }
    const bool safe = ran && exactPathObserved && !std::filesystem::exists(marker);
    std::error_code cleanupError;
    std::filesystem::remove_all(root, cleanupError);
    if (!safe) return 3;
#endif

    return 0;
}
