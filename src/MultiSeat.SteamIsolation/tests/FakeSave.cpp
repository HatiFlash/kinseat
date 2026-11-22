#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <sddl.h>
#include <cstdio>
#include <string>
#include <vector>

namespace {
constexpr wchar_t path[] = L"C:\\Users\\Public\\Documents\\Steam\\RUNE\\999999\\save.dat";
constexpr wchar_t directory[] = L"C:\\Users\\Public\\Documents\\Steam\\RUNE\\999999";
}
int wmain() {
    wchar_t user[256]{};
    DWORD userLength = 256;
    if (!GetUserNameW(user, &userLength)) return 1;
    SYSTEMTIME now{};
    GetSystemTime(&now);
    wchar_t line[512]{};
    swprintf_s(line, L"JOUEUR %s %04u-%02u-%02uT%02u:%02u:%02uZ\r\n",
        user, now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond);
    HANDLE output = CreateFileW(path, GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (output == INVALID_HANDLE_VALUE) { wprintf(L"WRITE_ERROR=%lu\n", GetLastError()); return 2; }
    DWORD written = 0;
    const DWORD bytes = static_cast<DWORD>(wcslen(line) * sizeof(wchar_t));
    BOOL ok = WriteFile(output, line, bytes, &written, nullptr);
    CloseHandle(output);
    if (!ok || written != bytes) return 3;
    HANDLE input = CreateFileW(path, GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (input == INVALID_HANDLE_VALUE) { wprintf(L"READ_ERROR=%lu\n", GetLastError()); return 4; }
    wchar_t readback[512]{};
    DWORD count = 0;
    ok = ReadFile(input, readback, sizeof(readback) - sizeof(wchar_t), &count, nullptr);
    if (!ok || count != bytes || wcscmp(readback, line) != 0) { CloseHandle(input); return 5; }
    std::vector<wchar_t> finalPath(32768);
    DWORD size = GetFinalPathNameByHandleW(input, finalPath.data(), static_cast<DWORD>(finalPath.size()), FILE_NAME_NORMALIZED);
    CloseHandle(input);
    if (!size || size >= finalPath.size()) return 6;
    wprintf(L"ACCOUNT=%s\nREADBACK=%sFINAL_PATH=%s\n", user, readback, finalPath.data());
    WIN32_FIND_DATAW entry{};
    std::wstring pattern = std::wstring(directory) + L"\\*";
    HANDLE search = FindFirstFileW(pattern.c_str(), &entry);
    if (search == INVALID_HANDLE_VALUE) { wprintf(L"ENUM_ERROR=%lu\n", GetLastError()); return 7; }
    do { wprintf(L"ENTRY=%s\n", entry.cFileName); } while (FindNextFileW(search, &entry));
    FindClose(search);
    return 0;
}
