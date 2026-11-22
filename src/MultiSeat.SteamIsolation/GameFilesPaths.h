#pragma once
#include <string>
#include <vector>

namespace game_files {
struct Redirect {
    std::wstring from, to, mode;
};
struct Manifest {
    std::wstring gameId, account, accountSid, profileDir, logFile;
    std::vector<Redirect> redirects;
    bool copyFromHostOnce = false;
};
// Empty result means invalid or unsupported path. cwd must be a drive-absolute path.
std::wstring Normalize(const std::wstring& path, const std::wstring& cwd);
bool Within(const std::wstring& path, const std::wstring& prefix);
std::wstring Rewrite(const std::wstring& path, const std::wstring& cwd,
                     const std::vector<Redirect>& rules, bool* changed = nullptr);
struct OverlayDecision {
    std::wstring original, overlay, relative;
    bool matched = false, useOverlay = false, copyUp = false, hidden = false;
};
OverlayDecision DecideOverlay(const std::wstring& path, const std::wstring& cwd,
    const std::vector<Redirect>& rules, bool writing, bool overlayExists, bool originalExists, bool deleted);
std::vector<std::wstring> MergeNames(const std::vector<std::wstring>& original,
    const std::vector<std::wstring>& overlay, const std::vector<std::wstring>& deleted);
bool ParseManifest(const std::string& utf8, Manifest& result, std::string& error);
}
