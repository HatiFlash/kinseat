#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <string>
#include <cstring>
#include <vector>

// Faux jeu façon Longshot : range ses sauvegardes dans <dossier du jeu>\user\save\<id>\slot_NN\progress.sav.
// Codes de retour : 0 OK, 1 module introuvable, 2 log, 3 création dossiers, 4 écriture progress.sav,
// 5 écriture log.txt, 6 LOCALAPPDATA indisponible, 7 ouverture du rapport, 8 écriture du rapport.

static bool Append(const std::wstring& path, const char* text) {
    HANDLE h = CreateFileW(path.c_str(), FILE_APPEND_DATA, FILE_SHARE_READ, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) return false;
    DWORD written = 0;
    bool ok = WriteFile(h, text, static_cast<DWORD>(strlen(text)), &written, nullptr) != FALSE;
    CloseHandle(h);
    return ok;
}
static std::string Read(const std::wstring& path) {
    HANDLE h = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_EXISTING, 0, nullptr);
    if (h == INVALID_HANDLE_VALUE) return "ERREUR ouverture cache_origine.bin\r\n";
    std::string bytes;
    char buffer[4096]; DWORD n = 0;
    while (ReadFile(h, buffer, sizeof(buffer), &n, nullptr) && n) bytes.append(buffer, n);
    CloseHandle(h);
    return bytes;
}
static std::string Utf8(const std::wstring& s) {
    int length = WideCharToMultiByte(CP_UTF8, 0, s.data(), static_cast<int>(s.size()), nullptr, 0, nullptr, nullptr);
    std::string result(length, '\0');
    if (length) WideCharToMultiByte(CP_UTF8, 0, s.data(), static_cast<int>(s.size()), result.data(), length, nullptr, nullptr);
    return result;
}

// Parcourt récursivement "dir" (chemin absolu, avec \ final) et collecte les chemins relatifs à
// base\user\save. Les dossiers reçoivent un \ final. Profondeur max 4.
static void Enumerate(const std::wstring& dir, const std::wstring& relative, int depth, std::vector<std::wstring>& out) {
    if (depth > 4) return;
    WIN32_FIND_DATAW entry{};
    HANDLE scan = FindFirstFileW((dir + L"*").c_str(), &entry);
    if (scan == INVALID_HANDLE_VALUE) return;
    do {
        std::wstring name = entry.cFileName;
        if (name == L"." || name == L"..") continue;
        bool isDirectory = (entry.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
        std::wstring rel = relative + name;
        if (isDirectory) {
            rel += L"\\";
            out.push_back(rel);
            Enumerate(dir + name + L"\\", rel, depth + 1, out);
        } else {
            out.push_back(rel);
        }
    } while (FindNextFileW(scan, &entry));
    FindClose(scan);
}

int wmain(int argc, wchar_t** argv) {
    wchar_t module[32768]{};
    if (!GetModuleFileNameW(nullptr, module, 32768)) return 1;
    std::wstring directory(module);
    directory.resize(directory.find_last_of(L"\\/"));
    std::wstring base = directory + L"\\";

    // Simule le relancement du vrai Longshot : seul le processus enfant effectue les I/O.
    if (argc > 2 && std::wstring(argv[2]) == L"--relaunch-suspended") {
        std::wstring command = L"\"" + std::wstring(module) + L"\" \"" + argv[1] + L"\"";
        STARTUPINFOW startup{}; startup.cb = sizeof(startup);
        PROCESS_INFORMATION child{};
        if (!CreateProcessW(module, command.data(), nullptr, nullptr, FALSE, CREATE_SUSPENDED,
                            nullptr, directory.c_str(), &startup, &child)) return 9;
        if (ResumeThread(child.hThread) == static_cast<DWORD>(-1)) {
            TerminateProcess(child.hProcess, 9);
            CloseHandle(child.hThread); CloseHandle(child.hProcess); return 9;
        }
        CloseHandle(child.hThread);
        DWORD code = 9;
        if (WaitForSingleObject(child.hProcess, INFINITE) != WAIT_OBJECT_0 ||
            !GetExitCodeProcess(child.hProcess, &code)) code = 9;
        CloseHandle(child.hProcess);
        return static_cast<int>(code);
    }

    std::string initial = Read(base + L"cache_origine.bin");

    // Sauvegardes vues avant écriture.
    std::vector<std::wstring> seen;
    const std::wstring saveRoot = base + L"user\\save";
    const DWORD saveAttributes = GetFileAttributesW(saveRoot.c_str());
    const bool saveExists = saveAttributes != INVALID_FILE_ATTRIBUTES && (saveAttributes & FILE_ATTRIBUTE_DIRECTORY);
    if (saveExists) Enumerate(saveRoot + L"\\", L"user\\save\\", 1, seen);

    if (!Append(base + L"log.txt", "faux longshot: lancement\r\n")) return 2;

    // Création de l'arborescence de sauvegarde (ERROR_ALREADY_EXISTS ignoré).
    const std::wstring folders[] = {
        base + L"user",
        base + L"user\\save",
        base + L"user\\save\\joueur",
        base + L"user\\save\\joueur\\slot_00",
    };
    for (const std::wstring& folder : folders) {
        if (!CreateDirectoryW(folder.c_str(), nullptr) && GetLastError() != ERROR_ALREADY_EXISTS) return 3;
    }
    if (!Append(base + L"user\\save\\joueur\\slot_00\\progress.sav", "faux longshot: nouvelle partie\r\n")) return 4;
    if (!Append(base + L"log.txt", "faux longshot: sauvegarde ecrite\r\n")) return 5;

    std::string names;
    if (!saveExists) {
        names = "user\\save absent\r\n";
    } else if (seen.empty()) {
        names = "(aucune)\r\n";
    } else {
        for (const std::wstring& item : seen) { names += Utf8(item); names += "\r\n"; }
    }

    std::wstring report;
    if (argc > 1) report = argv[1];
    else {
        wchar_t local[32768]{};
        DWORD n = GetEnvironmentVariableW(L"LOCALAPPDATA", local, 32768);
        if (!n || n >= 32768) return 6;
        std::wstring folder = std::wstring(local, n) + L"\\KinseatFake";
        CreateDirectoryW(folder.c_str(), nullptr);
        report = folder + L"\\longshot-report.txt";
    }
    HANDLE output = CreateFileW(report.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (output == INVALID_HANDLE_VALUE) return 7;
    std::string text = "sauvegardes vues avant ecriture:\r\n" + names + "contenu initial de cache_origine.bin:\r\n" + initial;
    DWORD written = 0;
    BOOL ok = WriteFile(output, text.data(), static_cast<DWORD>(text.size()), &written, nullptr);
    CloseHandle(output);
    return ok && written == text.size() ? 0 : 8;
}
