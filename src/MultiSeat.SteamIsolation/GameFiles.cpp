#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winternl.h>
#include <sddl.h>
#include <MinHook.h>
#include "GameFilesPaths.h"
#include "Inject.h"
#include <string>
#include <vector>
#include <fstream>
#include <mutex>
#include <cstddef>
#include <cstring>
#include <iterator>
#include <algorithm>
#include <unordered_map>
#include <map>

extern "C" IMAGE_DOS_HEADER __ImageBase;
namespace {
using namespace game_files;
Manifest manifest;
std::wstring dllPath;
std::wstring activeManifestPath;
LONG state = 0;
thread_local bool inside = false;
std::mutex logMutex;
struct Guard { bool previous = inside; Guard() { inside = true; } ~Guard() { inside = previous; } };
void Log(const std::wstring& text) {
    if (manifest.logFile.empty() || inside) return;
    Guard guard;
    std::lock_guard<std::mutex> lock(logMutex);
    WIN32_FILE_ATTRIBUTE_DATA attrs{};
    if (GetFileAttributesExW(manifest.logFile.c_str(), GetFileExInfoStandard, &attrs) &&
        (attrs.nFileSizeHigh || attrs.nFileSizeLow >= 1024 * 1024)) {
        MoveFileExW(manifest.logFile.c_str(), (manifest.logFile + L".1").c_str(), MOVEFILE_REPLACE_EXISTING);
    }
    HANDLE h = CreateFileW(manifest.logFile.c_str(), FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE,
                           nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) return;
    LARGE_INTEGER size{};
    if (GetFileSizeEx(h, &size) && size.QuadPart == 0) {
        DWORD written = 0;
        const BYTE bom[] = {0xef, 0xbb, 0xbf};
        WriteFile(h, bom, sizeof(bom), &written, nullptr);
    }
    auto line = text + L"\r\n";
    int len = WideCharToMultiByte(CP_UTF8, 0, line.c_str(), static_cast<int>(line.size()), nullptr, 0, nullptr, nullptr);
    if (len > 0) {
        std::string bytes(static_cast<size_t>(len), '\0');
        WideCharToMultiByte(CP_UTF8, 0, line.c_str(), static_cast<int>(line.size()), bytes.data(), len, nullptr, nullptr);
        DWORD written = 0; WriteFile(h, bytes.data(), static_cast<DWORD>(bytes.size()), &written, nullptr);
    }
    CloseHandle(h);
}
std::wstring Cwd() {
    std::vector<wchar_t> buffer(32768);
    DWORD n = GetCurrentDirectoryW(static_cast<DWORD>(buffer.size()), buffer.data());
    return n && n < buffer.size() ? std::wstring(buffer.data(), n) : L"";
}
std::wstring HandlePath(HANDLE root) {
    Guard guard;
    std::vector<wchar_t> buffer(32768);
    DWORD n = GetFinalPathNameByHandleW(root, buffer.data(), static_cast<DWORD>(buffer.size()), FILE_NAME_NORMALIZED);
    return n && n < buffer.size() ? std::wstring(buffer.data(), n) : L"";
}
void Parents(const std::wstring& target) {
    Guard guard;
    size_t end = target.find_last_of(L'\\');
    if (end == std::wstring::npos || end <= 2) return;
    std::wstring dir = target.substr(0, end);
    size_t pos = 3;
    while ((pos = dir.find(L'\\', pos)) != std::wstring::npos) {
        CreateDirectoryW(dir.substr(0, pos).c_str(), nullptr); ++pos;
    }
    if (!CreateDirectoryW(dir.c_str(), nullptr)) {
        DWORD error = GetLastError();
        if (error != ERROR_ALREADY_EXISTS) {
            // Logging is deferred to the caller; this thread is inside the recursion guard.
            OutputDebugStringW((L"GameFiles: cannot create directory " + dir + L" (" + std::to_wstring(error) + L")").c_str());
        }
    }
}
std::mutex deletedMutex;
std::wstring DeletedFile(const Redirect& rule) { return Normalize(rule.to, L"") + L"\\.kinseat-deleted"; }
std::string Utf8(const std::wstring& value) {
    int n = WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    if (n <= 0) return {};
    std::string bytes(static_cast<size_t>(n), '\0');
    WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), bytes.data(), n, nullptr, nullptr);
    return bytes;
}
std::wstring Key(std::wstring path) {
    std::transform(path.begin(), path.end(), path.begin(), [](wchar_t c) { return c >= L'A' && c <= L'Z' ? c + 32 : c; });
    return path;
}
const Redirect* OverlayRule(const std::wstring& path) {
    for (const auto& rule : manifest.redirects)
        if (rule.mode == L"overlay" && Within(path, Normalize(rule.from, L""))) return &rule;
    return nullptr;
}
// UTF-8 relative paths, one per line. Serialized inside this process.
bool Tombstone(const Redirect& rule, const std::wstring& relative, bool add) {
    if (relative.empty() || relative.find(L'\n') != std::wstring::npos || relative.find(L'\r') != std::wstring::npos) return false;
    Guard guard;
    std::lock_guard<std::mutex> lock(deletedMutex);
    std::wstring file = DeletedFile(rule);
    HANDLE h = CreateFileW(file.c_str(), GENERIC_READ | (add ? FILE_APPEND_DATA : 0),
        FILE_SHARE_READ, nullptr, add ? OPEN_ALWAYS : OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) return false;
    LARGE_INTEGER size{};
    std::string content;
    if (GetFileSizeEx(h, &size) && size.QuadPart >= 0 && size.QuadPart < 16 * 1024 * 1024) {
        content.resize(static_cast<size_t>(size.QuadPart));
        DWORD read = 0;
        if (!content.empty() && !ReadFile(h, content.data(), static_cast<DWORD>(content.size()), &read, nullptr)) content.clear();
        else content.resize(read);
    }
    auto name = Utf8(Key(relative));
    bool found = content.find("\n" + name + "\n") != std::string::npos ||
        content.compare(0, name.size() + 1, name + "\n") == 0;
    if (add && !found) {
        std::string entry = name + "\n";
        DWORD written = 0;
        WriteFile(h, entry.data(), static_cast<DWORD>(entry.size()), &written, nullptr);
    }
    CloseHandle(h);
    return found;
}
bool Exists(const std::wstring& path) {
    Guard guard;
    return GetFileAttributesW(path.c_str()) != INVALID_FILE_ATTRIBUTES;
}
struct Name {
    OBJECT_ATTRIBUTES attributes{};
    UNICODE_STRING unicode{};
    std::wstring storage;
    bool hidden = false;
    bool failed = false;
    OverlayDecision overlay;
    POBJECT_ATTRIBUTES Apply(POBJECT_ATTRIBUTES original, bool makeParents, const wchar_t* operation, bool writing = false) {
        if (inside || !original || !original->ObjectName || !original->ObjectName->Buffer) return original;
        auto& name = *original->ObjectName;
        std::wstring raw(name.Buffer, name.Length / sizeof(wchar_t));
        if (original->RootDirectory && !raw.empty() && raw[0] != L'\\' && (raw.size() < 2 || raw[1] != L':')) {
            auto root = HandlePath(original->RootDirectory);
            if (root.empty()) return original;
            raw = root + L"\\" + raw;
        }
        bool changed = false;
        auto target = Rewrite(raw, Cwd(), manifest.redirects, &changed);
        if (!changed) {
            auto normalized = Normalize(raw, Cwd());
            auto rule = OverlayRule(normalized);
            if (rule && normalized != Normalize(rule->from, L"")) {
                auto relative = normalized.substr(Normalize(rule->from, L"").size());
                if (Key(relative) == L"\\.kinseat-deleted") { hidden = true; return original; }
                auto candidate = Normalize(rule->to, L"") + relative;
                bool exists = Exists(candidate);
                bool originalExists = Exists(normalized);
                bool deleted = Tombstone(*rule, relative, false);
                overlay = DecideOverlay(raw, Cwd(), manifest.redirects, writing, exists, originalExists, deleted);
                hidden = overlay.hidden;
                if (hidden && !writing) return original;
                if (overlay.copyUp) {
                    Parents(candidate);
                    WIN32_FILE_ATTRIBUTE_DATA attrs{};
                    bool gotAttrs = false;
                    { Guard guard; gotAttrs = GetFileAttributesExW(normalized.c_str(), GetFileExInfoStandard, &attrs) != FALSE; }
                    if (gotAttrs && (attrs.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY)) {
                        Guard guard;
                        if (!CreateDirectoryW(candidate.c_str(), nullptr) && GetLastError() != ERROR_ALREADY_EXISTS) failed = true;
                    } else {
                        if (attrs.nFileSizeHigh || attrs.nFileSizeLow > 0x80000000)
                            Log(L"Copy-up volumineux : " + normalized + L" (" +
                                std::to_wstring((static_cast<unsigned long long>(attrs.nFileSizeHigh) << 32) | attrs.nFileSizeLow) + L" octets)");
                        Guard guard;
                        if (!CopyFileW(normalized.c_str(), candidate.c_str(), TRUE) && GetLastError() != ERROR_FILE_EXISTS) failed = true;
                    }
                }
                if (overlay.useOverlay && !failed) { changed = true; target = candidate; }
            }
        }
        if (!changed || target.size() > 32760) return original;
        if (makeParents) Parents(target);
        Log(std::wstring(operation) + L": " + raw + L" -> " + target);
        storage = L"\\??\\" + target;
        unicode.Buffer = storage.data();
        unicode.Length = static_cast<USHORT>(storage.size() * sizeof(wchar_t));
        unicode.MaximumLength = static_cast<USHORT>((storage.size() + 1) * sizeof(wchar_t));
        attributes = *original; attributes.RootDirectory = nullptr; attributes.ObjectName = &unicode;
        return &attributes;
    }
};
using OpenFn = NTSTATUS (NTAPI*)(PHANDLE, ACCESS_MASK, POBJECT_ATTRIBUTES, PIO_STATUS_BLOCK, ULONG, ULONG);
using CreateFn = NTSTATUS (NTAPI*)(PHANDLE, ACCESS_MASK, POBJECT_ATTRIBUTES, PIO_STATUS_BLOCK, PLARGE_INTEGER, ULONG, ULONG, ULONG, ULONG, PVOID, ULONG);
using AttrFn = NTSTATUS (NTAPI*)(POBJECT_ATTRIBUTES, PVOID);
using FullAttrFn = NTSTATUS (NTAPI*)(POBJECT_ATTRIBUTES, PVOID);
using DeleteFn = NTSTATUS (NTAPI*)(POBJECT_ATTRIBUTES);
using SetFn = NTSTATUS (NTAPI*)(HANDLE, PIO_STATUS_BLOCK, PVOID, ULONG, FILE_INFORMATION_CLASS);
CreateFn createOriginal = nullptr; OpenFn openOriginal = nullptr;
AttrFn attrOriginal = nullptr; FullAttrFn fullOriginal = nullptr;
DeleteFn deleteOriginal = nullptr; SetFn setOriginal = nullptr;
NTSTATUS NTAPI Create(PHANDLE h, ACCESS_MASK access, POBJECT_ATTRIBUTES a, PIO_STATUS_BLOCK io, PLARGE_INTEGER allocation,
                       ULONG attributes, ULONG sharing, ULONG disposition, ULONG options, PVOID ea, ULONG eaLength) {
    Name n; auto target = n.Apply(a, true, L"création", (access & (FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_WRITE_ATTRIBUTES | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_WRITE)) || disposition != FILE_OPEN || (options & FILE_DELETE_ON_CLOSE));
    if (n.failed) return static_cast<NTSTATUS>(0xC0000022);
    if (n.hidden && (!n.overlay.matched || disposition == FILE_OPEN || disposition == FILE_OVERWRITE)) return static_cast<NTSTATUS>(0xC0000034);
    auto status = createOriginal(h, access, target, io, allocation, attributes, sharing, disposition, options, ea, eaLength);
    if (status < 0 && target != a && status != static_cast<NTSTATUS>(0xC0000035) &&
        status != static_cast<NTSTATUS>(0xC0000034) && status != static_cast<NTSTATUS>(0xC000003A)) {
        wchar_t code[11]{};
        swprintf_s(code, L"0x%08X", static_cast<unsigned long>(status));
        Log(L"Erreur NtCreateFile : " + std::wstring(code));
    }
    return status;
}
NTSTATUS NTAPI Open(PHANDLE h, ACCESS_MASK access, POBJECT_ATTRIBUTES a, PIO_STATUS_BLOCK io, ULONG sharing, ULONG options) {
    Name n; auto target = n.Apply(a, false, L"ouverture", (access & (FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_WRITE_ATTRIBUTES | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_WRITE)) || (options & FILE_DELETE_ON_CLOSE));
    if (n.hidden) return static_cast<NTSTATUS>(0xC0000034);
    if (n.failed) return static_cast<NTSTATUS>(0xC0000022);
    auto status = openOriginal(h, access, target, io, sharing, options);
    if (status < 0 && target != a) Log(L"Erreur NtOpenFile : " + std::to_wstring(static_cast<unsigned long>(status)));
    return status;
}
NTSTATUS NTAPI Attr(POBJECT_ATTRIBUTES a, PVOID info) {
    Name n; auto target = n.Apply(a, false, L"attributs");
    return n.hidden ? static_cast<NTSTATUS>(0xC0000034) : attrOriginal(target, info);
}
NTSTATUS NTAPI Full(POBJECT_ATTRIBUTES a, PVOID info) {
    Name n; auto target = n.Apply(a, false, L"attributs complets");
    return n.hidden ? static_cast<NTSTATUS>(0xC0000034) : fullOriginal(target, info);
}
NTSTATUS NTAPI Delete(POBJECT_ATTRIBUTES a) {
    Name n; auto target = n.Apply(a, false, L"suppression", true);
    if (n.failed) return static_cast<NTSTATUS>(0xC0000022);
    if (n.hidden) return static_cast<NTSTATUS>(0xC0000034);
    auto status = deleteOriginal(target);
    if (status >= 0 && n.overlay.matched) {
        auto rule = OverlayRule(n.overlay.original);
        if (rule && Exists(n.overlay.original)) Tombstone(*rule, n.overlay.relative, true);
    }
    return status;
}
// The rename and hard-link records start with ReplaceIfExists/Flags, RootDirectory and FileNameLength.
// Their filename is a variable-length UTF-16 string (not NUL-terminated), including Ex variants.
struct TargetName { ULONG flags; HANDLE root; ULONG bytes; wchar_t name[1]; };
NTSTATUS NTAPI Set(HANDLE file, PIO_STATUS_BLOCK io, PVOID data, ULONG length, FILE_INFORMATION_CLASS kind) {
    const int code = static_cast<int>(kind);
    std::wstring source = inside ? L"" : Normalize(HandlePath(file), L"");
    const Redirect* sourceRule = OverlayRule(source);
    // Handles opened before injection must never mutate the original game directory.
    if (!inside && sourceRule && (code == 10 || code == 65 || code == 13 || code == 64 || code == 11 || code == 72))
        return static_cast<NTSTATUS>(0xC0000022);
    const Redirect* overlaySource = nullptr;
    std::wstring sourceRelative;
    for (const auto& rule : manifest.redirects) if (rule.mode == L"overlay" &&
        Within(source, Normalize(rule.to, L""))) {
        overlaySource = &rule;
        sourceRelative = source.substr(Normalize(rule.to, L"").size());
        break;
    }
    if (!inside && overlaySource && (code == 13 || code == 64)) {
        auto status = setOriginal(file, io, data, length, kind);
        if (status >= 0 && !sourceRelative.empty()) {
            auto originalPath = Normalize(overlaySource->from, L"") + sourceRelative;
            if (Exists(originalPath)) Tombstone(*overlaySource, sourceRelative, true);
        }
        return status;
    }
    if (!inside && data && (code == 10 || code == 11 || code == 65 || code == 72) &&
        length >= offsetof(TargetName, name)) {
        auto* original = static_cast<TargetName*>(data);
        const size_t offset = offsetof(TargetName, name);
        if (original->bytes <= length - offset && !(original->bytes % sizeof(wchar_t))) {
            std::wstring path(original->name, original->bytes / sizeof(wchar_t));
            if (original->root && !path.empty() && path[0] != L'\\' && (path.size() < 2 || path[1] != L':')) {
                auto root = HandlePath(original->root);
                if (!root.empty()) path = root + L"\\" + path;
            }
            bool changed = false;
            auto target = Rewrite(path, Cwd(), manifest.redirects, &changed);
            if (!changed) {
                auto normalized = Normalize(path, Cwd());
                auto rule = OverlayRule(normalized);
                if (rule) {
                    target = Normalize(rule->to, L"") + normalized.substr(Normalize(rule->from, L"").size());
                    changed = true;
                }
            }
            if (changed && target.size() < 32760) {
                Parents(target);
                Log(L"renommage/lien : " + path + L" -> " + target);
                std::wstring native = L"\\??\\" + target;
                std::vector<BYTE> replacement(offset + native.size() * sizeof(wchar_t));
                auto* record = reinterpret_cast<TargetName*>(replacement.data());
                record->flags = original->flags; record->root = nullptr;
                record->bytes = static_cast<ULONG>(native.size() * sizeof(wchar_t));
                memcpy(replacement.data() + offset, native.data(), record->bytes);
                auto status = setOriginal(file, io, replacement.data(), static_cast<ULONG>(replacement.size()), kind);
                if (status >= 0 && overlaySource && (code == 10 || code == 65) && !sourceRelative.empty() &&
                    Exists(Normalize(overlaySource->from, L"") + sourceRelative))
                    Tombstone(*overlaySource, sourceRelative, true);
                return status;
            }
        }
    }
    return setOriginal(file, io, data, length, kind);
}
using QueryFn = NTSTATUS (NTAPI*)(HANDLE, HANDLE, PIO_APC_ROUTINE, PVOID, PIO_STATUS_BLOCK, PVOID,
    ULONG, FILE_INFORMATION_CLASS, BOOLEAN, PUNICODE_STRING, BOOLEAN);
using QueryExFn = NTSTATUS (NTAPI*)(HANDLE, HANDLE, PIO_APC_ROUTINE, PVOID, PIO_STATUS_BLOCK, PVOID,
    ULONG, FILE_INFORMATION_CLASS, ULONG, PUNICODE_STRING);
using CloseFn = NTSTATUS (NTAPI*)(HANDLE);
QueryFn queryOriginal = nullptr;
QueryExFn queryExOriginal = nullptr;
CloseFn closeOriginal = nullptr;
struct Entry { std::wstring name; WIN32_FIND_DATAW data{}; };
struct Listing { std::vector<Entry> entries; size_t next = 0; std::wstring directory; };
// NtClose stays hooked during CRT teardown. Keep its state alive until the process exits:
// a global mutex/map destructor can otherwise run before the final handles are closed.
std::mutex& listingMutex = *new std::mutex;
std::unordered_map<HANDLE, Listing>& listings = *new std::unordered_map<HANDLE, Listing>;
bool Wildcard(const wchar_t* pattern, const wchar_t* name) {
    if (!pattern || !*pattern) return !*name;
    if (pattern[0] == L'*' && pattern[1] == L'.' && pattern[2] == L'*' && !pattern[3]) return true;
    if (*pattern == L'*') return Wildcard(pattern + 1, name) || (*name && Wildcard(pattern, name + 1));
    auto fold = [](wchar_t c) { return c >= L'A' && c <= L'Z' ? c + 32 : c; };
    return *name && (*pattern == L'?' || fold(*pattern) == fold(*name)) && Wildcard(pattern + 1, name + 1);
}
std::vector<Entry> Scan(const std::wstring& dir, const std::wstring& pattern) {
    std::vector<Entry> result;
    WIN32_FIND_DATAW data{};
    Guard guard;
    HANDLE scan = FindFirstFileW((dir + L"\\*").c_str(), &data);
    if (scan == INVALID_HANDLE_VALUE) return result;
    do {
        if (Wildcard(pattern.c_str(), data.cFileName)) result.push_back({data.cFileName, data});
    } while (FindNextFileW(scan, &data));
    FindClose(scan);
    return result;
}
bool DirectoryPaths(const std::wstring& path, std::wstring& original, std::wstring& overlay, const Redirect*& rule) {
    for (const auto& candidate : manifest.redirects) {
        if (candidate.mode != L"overlay") continue;
        auto from = Normalize(candidate.from, L""), to = Normalize(candidate.to, L"");
        if (Within(path, from)) {
            original = path; overlay = to + path.substr(from.size()); rule = &candidate; return true;
        }
        if (Within(path, to)) {
            overlay = path; original = from + path.substr(to.size()); rule = &candidate; return true;
        }
    }
    return false;
}
Listing Collect(const std::wstring& path, const std::wstring& pattern) {
    Listing list;
    std::wstring original, overlay;
    const Redirect* rule = nullptr;
    if (!DirectoryPaths(path, original, overlay, rule)) return list;
    list.directory = path;
    auto o = Scan(original, pattern), v = Scan(overlay, pattern);
    std::vector<std::wstring> on, vn, deleted;
    for (const auto& entry : o) on.push_back(entry.name);
    for (const auto& entry : v) vn.push_back(entry.name);
    for (const auto& entry : o) {
        auto relative = original.substr(Normalize(rule->from, L"").size()) + L"\\" + entry.name;
        if (Tombstone(*rule, relative, false)) deleted.push_back(entry.name);
    }
    auto names = MergeNames(on, vn, deleted);
    for (const auto& name : names) {
        auto found = std::find_if(v.begin(), v.end(), [&](const Entry& e) { return Key(e.name) == Key(name); });
        if (found != v.end()) list.entries.push_back(*found);
        else {
            found = std::find_if(o.begin(), o.end(), [&](const Entry& e) { return Key(e.name) == Key(name); });
            if (found != o.end()) list.entries.push_back(*found);
        }
    }
    return list;
}
size_t NameOffset(int kind) {
    switch (kind) {
    case 1: return 64; // FileDirectoryInformation
    case 2: return 68; // FileFullDirectoryInformation
    case 3: return 94; // FileBothDirectoryInformation
    case 12: return 12; // FileNamesInformation
    case 37: return 104; // FileIdBothDirectoryInformation
    case 38: return 80; // FileIdFullDirectoryInformation
    default: return 0;
    }
}
void Put32(BYTE* dst, size_t offset, DWORD value) { memcpy(dst + offset, &value, sizeof(value)); }
void Put64(BYTE* dst, size_t offset, ULONGLONG value) { memcpy(dst + offset, &value, sizeof(value)); }
ULONGLONG Time(const FILETIME& t) { return (static_cast<ULONGLONG>(t.dwHighDateTime) << 32) | t.dwLowDateTime; }
NTSTATUS Serve(HANDLE h, PIO_STATUS_BLOCK io, PVOID buffer, ULONG length, FILE_INFORMATION_CLASS kind,
               bool single, PUNICODE_STRING mask, bool restart) {
    auto path = Normalize(HandlePath(h), L"");
    std::wstring original, overlay;
    const Redirect* rule = nullptr;
    if (!DirectoryPaths(path, original, overlay, rule)) return static_cast<NTSTATUS>(0xC0000001);
    std::wstring pattern = L"*";
    if (mask && mask->Buffer && mask->Length) pattern.assign(mask->Buffer, mask->Length / sizeof(wchar_t));
    std::lock_guard<std::mutex> lock(listingMutex);
    auto it = listings.find(h);
    if (restart || it == listings.end() || it->second.directory != path)
        it = listings.insert_or_assign(h, Collect(path, pattern)).first;
    auto& listing = it->second;
    if (listing.next == listing.entries.size()) {
        io->Information = 0;
        io->Status = listing.next ? static_cast<NTSTATUS>(0x80000006) : static_cast<NTSTATUS>(0xC000000F);
        return io->Status;
    }
    BYTE* output = static_cast<BYTE*>(buffer);
    size_t used = 0, previous = 0;
    auto offset = NameOffset(static_cast<int>(kind));
    while (listing.next < listing.entries.size()) {
        const auto& entry = listing.entries[listing.next];
        size_t nameBytes = entry.name.size() * sizeof(wchar_t);
        size_t size = (offset + nameBytes + 7) & ~size_t{7};
        if (size > length - used) {
            if (!used) { io->Information = 0; io->Status = static_cast<NTSTATUS>(0x80000005); return io->Status; }
            break;
        }
        memset(output + used, 0, size);
        if (used) Put32(output, previous, static_cast<DWORD>(used - previous));
        Put32(output, used + 4, listing.next < 0xffffffffu ? static_cast<DWORD>(listing.next) : 0);
        if (offset != 12) {
            const auto& data = entry.data;
            Put64(output, used + 8, Time(data.ftCreationTime));
            Put64(output, used + 16, Time(data.ftLastAccessTime));
            Put64(output, used + 24, Time(data.ftLastWriteTime));
            Put64(output, used + 32, Time(data.ftLastWriteTime));
            auto bytes = (static_cast<ULONGLONG>(data.nFileSizeHigh) << 32) | data.nFileSizeLow;
            Put64(output, used + 40, bytes);
            Put64(output, used + 48, bytes);
            Put32(output, used + 56, data.dwFileAttributes);
            Put32(output, used + 60, static_cast<DWORD>(nameBytes));
            if (offset >= 68) Put32(output, used + 64, 0); // EA size
        } else Put32(output, used + 8, static_cast<DWORD>(nameBytes));
        memcpy(output + used + offset, entry.name.data(), nameBytes);
        previous = used; used += size; ++listing.next;
        if (single) break;
    }
    io->Information = used; io->Status = 0;
    return 0;
}
NTSTATUS NTAPI Query(HANDLE h, HANDLE event, PIO_APC_ROUTINE apc, PVOID context, PIO_STATUS_BLOCK io,
    PVOID buffer, ULONG length, FILE_INFORMATION_CLASS kind, BOOLEAN single, PUNICODE_STRING mask, BOOLEAN restart) {
    if (inside || !NameOffset(static_cast<int>(kind)) || !io || !buffer || event || apc || !queryOriginal)
        return queryOriginal(h, event, apc, context, io, buffer, length, kind, single, mask, restart);
    auto path = Normalize(HandlePath(h), L"");
    std::wstring original, overlay; const Redirect* rule = nullptr;
    if (!DirectoryPaths(path, original, overlay, rule))
        return queryOriginal(h, event, apc, context, io, buffer, length, kind, single, mask, restart);
    return Serve(h, io, buffer, length, kind, single != FALSE, mask, restart != FALSE);
}
NTSTATUS NTAPI QueryEx(HANDLE h, HANDLE event, PIO_APC_ROUTINE apc, PVOID context, PIO_STATUS_BLOCK io,
    PVOID buffer, ULONG length, FILE_INFORMATION_CLASS kind, ULONG flags, PUNICODE_STRING mask) {
    if (inside || !NameOffset(static_cast<int>(kind)) || !io || !buffer || event || apc || !queryExOriginal || (flags & ~3u))
        return queryExOriginal(h, event, apc, context, io, buffer, length, kind, flags, mask);
    auto path = Normalize(HandlePath(h), L"");
    std::wstring original, overlay; const Redirect* rule = nullptr;
    if (!DirectoryPaths(path, original, overlay, rule))
        return queryExOriginal(h, event, apc, context, io, buffer, length, kind, flags, mask);
    return Serve(h, io, buffer, length, kind, (flags & 2u) != 0, mask, (flags & 1u) != 0);
}
NTSTATUS NTAPI Close(HANDLE h) {
    if (!inside) { std::lock_guard<std::mutex> lock(listingMutex); listings.erase(h); }
    return closeOriginal(h);
}
using CreateInternal = BOOL (WINAPI*)(HANDLE, LPCWSTR, LPWSTR, LPSECURITY_ATTRIBUTES, LPSECURITY_ATTRIBUTES,
    BOOL, DWORD, LPVOID, LPCWSTR, LPSTARTUPINFOW, LPPROCESS_INFORMATION, PHANDLE);
CreateInternal createInternal = nullptr;
// A caller-supplied environment replaces inheritance altogether. Reject it before creating any
// child unless it already contains the exact manifest used to initialize this DLL. This deliberately
// refuses missing/mismatched explicit blocks instead of silently running an unisolated game.
bool HasManifest(LPVOID environment, DWORD flags) {
    if (!environment) return true;
    constexpr wchar_t key[] = L"MULTISEAT_GAME_MANIFEST";
    constexpr size_t limit = 32767; // Windows environment block maximum (in characters).
    size_t offset = 0;
    if (flags & CREATE_UNICODE_ENVIRONMENT) {
        const auto* block = static_cast<const wchar_t*>(environment);
        while (offset < limit && block[offset]) {
            size_t end = offset;
            while (end < limit && block[end]) ++end;
            if (end == limit) return false;
            std::wstring entry(block + offset, end - offset);
            if (entry.size() > sizeof(key) / sizeof(wchar_t) &&
                _wcsnicmp(entry.c_str(), key, (sizeof(key) / sizeof(wchar_t)) - 1) == 0 &&
                entry[(sizeof(key) / sizeof(wchar_t)) - 1] == L'=' &&
                entry.substr(sizeof(key) / sizeof(wchar_t)) == activeManifestPath) return true;
            offset = end + 1;
        }
    } else {
        const auto* block = static_cast<const char*>(environment);
        while (offset < limit && block[offset]) {
            size_t end = offset;
            while (end < limit && block[end]) ++end;
            if (end == limit) return false;
            int length = MultiByteToWideChar(CP_ACP, MB_ERR_INVALID_CHARS, block + offset,
                                             static_cast<int>(end - offset), nullptr, 0);
            if (!length) return false;
            std::wstring entry(static_cast<size_t>(length), L'\0');
            if (!MultiByteToWideChar(CP_ACP, MB_ERR_INVALID_CHARS, block + offset,
                                     static_cast<int>(end - offset), entry.data(), length)) return false;
            if (entry.size() > sizeof(key) / sizeof(wchar_t) &&
                _wcsnicmp(entry.c_str(), key, (sizeof(key) / sizeof(wchar_t)) - 1) == 0 &&
                entry[(sizeof(key) / sizeof(wchar_t)) - 1] == L'=' &&
                entry.substr(sizeof(key) / sizeof(wchar_t)) == activeManifestPath) return true;
            offset = end + 1;
        }
    }
    return false;
}
BOOL WINAPI Child(HANDLE token, LPCWSTR app, LPWSTR command, LPSECURITY_ATTRIBUTES processAttrs, LPSECURITY_ATTRIBUTES threadAttrs,
                  BOOL inherit, DWORD flags, LPVOID environment, LPCWSTR cwd, LPSTARTUPINFOW startup, LPPROCESS_INFORMATION info, PHANDLE newToken) {
    if (inside) return createInternal(token, app, command, processAttrs, threadAttrs, inherit, flags, environment, cwd, startup, info, newToken);
    if (!info || !HasManifest(environment, flags)) {
        Log(L"Création enfant refusée : manifeste absent ou différent dans l'environnement explicite");
        SetLastError(ERROR_DLL_INIT_FAILED); return FALSE;
    }
    BOOL ok = createInternal(token, app, command, processAttrs, threadAttrs, inherit, flags | CREATE_SUSPENDED,
                             environment, cwd, startup, info, newToken);
    if (!ok) return FALSE;
    auto refuse = [&](const std::wstring& error) {
        Log(L"Injection enfant impossible : " + error);
        TerminateProcess(info->hProcess, 1);
        CloseHandle(info->hThread); CloseHandle(info->hProcess); *info = {};
        SetLastError(ERROR_DLL_INIT_FAILED); return FALSE;
    };
    BOOL childWow = FALSE, ownWow = FALSE;
    if (!IsWow64Process(info->hProcess, &childWow) || !IsWow64Process(GetCurrentProcess(), &ownWow))
        return refuse(L"architecture impossible à vérifier");
    if (childWow != ownWow) {
        // No matching-architecture DLL/helper in this process: this limit remains best-effort.
        Log(L"Injection enfant impossible : architecture différente");
        if (!(flags & CREATE_SUSPENDED)) ResumeThread(info->hThread);
        return TRUE;
    }
    std::wstring error;
    bool injected = false;
    try { Guard guard; injected = steam_isolation::Inject(info->hProcess, dllPath, error); }
    catch (...) { error = L"allocation de ressources impossible"; }
    if (!injected) return refuse(error);
    if (!(flags & CREATE_SUSPENDED) && ResumeThread(info->hThread) == static_cast<DWORD>(-1))
        return refuse(L"reprise du processus impossible (" + std::to_wstring(GetLastError()) + L")");
    return TRUE;
}
template<class T> bool Hook(const wchar_t* module, const char* symbol, void* replacement, T* original) {
    HMODULE h = GetModuleHandleW(module);
    auto address = h ? GetProcAddress(h, symbol) : nullptr;
    return address && MH_CreateHook(reinterpret_cast<void*>(address), replacement, reinterpret_cast<void**>(original)) == MH_OK;
}
}
#if defined(_MSC_VER) && !defined(_WIN64)
#pragma comment(linker, "/EXPORT:MultiSeatInitialize=_MultiSeatInitialize@4")
#endif
extern "C" __declspec(dllexport) DWORD WINAPI MultiSeatInitialize(void*) {
    if (InterlockedCompareExchange(&state, 1, 0) != 0) return state == 2 ? 1 : 0;
    wchar_t manifestPath[32768];
    DWORD size = GetEnvironmentVariableW(L"MULTISEAT_GAME_MANIFEST", manifestPath, 32768);
    if (!size || size >= 32768) return 0;
    std::ifstream stream(manifestPath, std::ios::binary);
    if (!stream) return 0;
    std::string json((std::istreambuf_iterator<char>(stream)), std::istreambuf_iterator<char>());
    std::string error;
    if (json.size() > 1024 * 1024 || !ParseManifest(json, manifest, error)) return 0;
    HANDLE token = nullptr;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) return 0;
    DWORD needed = 0; GetTokenInformation(token, TokenUser, nullptr, 0, &needed);
    std::vector<BYTE> sidData(needed);
    bool sidOk = GetTokenInformation(token, TokenUser, sidData.data(), needed, &needed) != FALSE;
    CloseHandle(token);
    LPWSTR sid = nullptr;
    if (!sidOk || !ConvertSidToStringSidW(reinterpret_cast<TOKEN_USER*>(sidData.data())->User.Sid, &sid)) return 0;
    bool match = _wcsicmp(sid, manifest.accountSid.c_str()) == 0;
    LocalFree(sid);
    if (!match) return 0;
    std::vector<wchar_t> module(32768);
    DWORD n = GetModuleFileNameW(reinterpret_cast<HMODULE>(&__ImageBase), module.data(), static_cast<DWORD>(module.size()));
    if (!n || n >= module.size()) return 0;
    dllPath.assign(module.data(), n);
    activeManifestPath.assign(manifestPath, size);
    if (MH_Initialize() != MH_OK) return 0;
    bool ok = Hook(L"ntdll.dll", "NtCreateFile", reinterpret_cast<void*>(Create), &createOriginal)
        && Hook(L"ntdll.dll", "NtOpenFile", reinterpret_cast<void*>(Open), &openOriginal)
        && Hook(L"ntdll.dll", "NtQueryAttributesFile", reinterpret_cast<void*>(Attr), &attrOriginal)
        && Hook(L"ntdll.dll", "NtQueryFullAttributesFile", reinterpret_cast<void*>(Full), &fullOriginal)
        && Hook(L"ntdll.dll", "NtDeleteFile", reinterpret_cast<void*>(Delete), &deleteOriginal)
        && Hook(L"ntdll.dll", "NtSetInformationFile", reinterpret_cast<void*>(Set), &setOriginal)
        && Hook(L"ntdll.dll", "NtQueryDirectoryFile", reinterpret_cast<void*>(Query), &queryOriginal)
        && Hook(L"ntdll.dll", "NtQueryDirectoryFileEx", reinterpret_cast<void*>(QueryEx), &queryExOriginal)
        && Hook(L"ntdll.dll", "NtClose", reinterpret_cast<void*>(Close), &closeOriginal);
    // Child propagation is best-effort: an incompatible child architecture needs a separate helper.
    if (ok) Hook(L"kernelbase.dll", "CreateProcessInternalW", reinterpret_cast<void*>(Child), &createInternal);
    if (!ok || MH_EnableHook(MH_ALL_HOOKS) != MH_OK) { MH_Uninitialize(); return 0; }
    InterlockedExchange(&state, 2); Log(L"Redirection de fichiers active"); return 1;
}
BOOL WINAPI DllMain(HINSTANCE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) DisableThreadLibraryCalls(module);
    return TRUE;
}
