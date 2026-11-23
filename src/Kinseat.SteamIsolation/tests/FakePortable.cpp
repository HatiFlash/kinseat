#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <string>
#include <cstring>
#include <vector>

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
int wmain(int argc, wchar_t** argv) {
    wchar_t module[32768]{};
    if (!GetModuleFileNameW(nullptr, module, 32768)) return 1;
    std::wstring directory(module);
    directory.resize(directory.find_last_of(L"\\/"));
    std::wstring base = directory + L"\\";
    std::string initial = Read(base + L"cache_origine.bin");
    if (!Append(base + L"log.txt", "faux jeu: lancement\r\n") ||
        !Append(base + L"cache.bin", "faux jeu: cache\r\n")) return 2;
    CreateDirectoryW((base + L"user").c_str(), nullptr);
    if (!Append(base + L"user\\save.dat", "faux jeu: sauvegarde\r\n")) return 3;
    if (!Append(base + L"cache_origine.bin", "faux jeu: modification\r\n")) return 4;
    std::string names;
    WIN32_FIND_DATAW entry{};
    HANDLE scan = FindFirstFileW((base + L"*").c_str(), &entry);
    if (scan == INVALID_HANDLE_VALUE) return 5;
    do { names += Utf8(entry.cFileName); names += "\r\n"; } while (FindNextFileW(scan, &entry));
    FindClose(scan);
    std::wstring report;
    if (argc > 1) report = argv[1];
    else {
        wchar_t local[32768]{};
        DWORD n = GetEnvironmentVariableW(L"LOCALAPPDATA", local, 32768);
        if (!n || n >= 32768) return 6;
        std::wstring folder = std::wstring(local, n) + L"\\KinseatFake";
        CreateDirectoryW(folder.c_str(), nullptr);
        report = folder + L"\\report.txt";
    }
    HANDLE output = CreateFileW(report.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (output == INVALID_HANDLE_VALUE) return 7;
    std::string text = "liste vue:\r\n" + names + "contenu initial de cache_origine.bin:\r\n" + initial;
    DWORD written = 0;
    BOOL ok = WriteFile(output, text.data(), static_cast<DWORD>(text.size()), &written, nullptr);
    CloseHandle(output);
    return ok && written == text.size() ? 0 : 8;
}
