#include "GameFilesPaths.h"
#include <algorithm>
#include <cctype>
#include <codecvt>
#include <locale>
#include <map>
#include <stdexcept>
#include <utility>

namespace game_files {
namespace {
wchar_t Lower(wchar_t c) { return c >= L'A' && c <= L'Z' ? c - L'A' + L'a' : c; }
bool Drive(const std::wstring& s) {
    return s.size() >= 3 && ((s[0] >= L'A' && s[0] <= L'Z') || (s[0] >= L'a' && s[0] <= L'z')) && s[1] == L':' && s[2] == L'\\';
}
struct Value {
    enum Kind { String, Object, Array, Bool, Other } kind = Other;
    std::wstring text;
    bool boolean = false;
    std::map<std::wstring, Value> object;
    std::vector<Value> array;
};
class Parser {
    const std::string& input;
    size_t pos = 0;
    void Space() { while (pos < input.size() && (input[pos] == ' ' || input[pos] == '\n' || input[pos] == '\r' || input[pos] == '\t')) ++pos; }
    bool Eat(char c) { Space(); if (pos < input.size() && input[pos] == c) { ++pos; return true; } return false; }
    void Need(char c) { if (!Eat(c)) throw std::runtime_error("invalid JSON delimiter"); }
    unsigned Hex() {
        if (pos + 4 > input.size()) throw std::runtime_error("short Unicode escape");
        unsigned result = 0;
        for (int i = 0; i < 4; ++i) {
            const char c = input[pos++];
            result *= 16;
            if (c >= '0' && c <= '9') result += c - '0';
            else if (c >= 'a' && c <= 'f') result += c - 'a' + 10;
            else if (c >= 'A' && c <= 'F') result += c - 'A' + 10;
            else throw std::runtime_error("invalid Unicode escape");
        }
        return result;
    }
    static void Utf8(std::string& out, unsigned cp) {
        if (cp < 0x80) out.push_back(static_cast<char>(cp));
        else if (cp < 0x800) { out.push_back(static_cast<char>(0xc0 | (cp >> 6))); out.push_back(static_cast<char>(0x80 | (cp & 63))); }
        else if (cp < 0x10000) { out.push_back(static_cast<char>(0xe0 | (cp >> 12))); out.push_back(static_cast<char>(0x80 | ((cp >> 6) & 63))); out.push_back(static_cast<char>(0x80 | (cp & 63))); }
        else { out.push_back(static_cast<char>(0xf0 | (cp >> 18))); out.push_back(static_cast<char>(0x80 | ((cp >> 12) & 63))); out.push_back(static_cast<char>(0x80 | ((cp >> 6) & 63))); out.push_back(static_cast<char>(0x80 | (cp & 63))); }
    }
    std::wstring StringValue() {
        Need('"'); std::string bytes; bool closed = false;
        while (pos < input.size()) {
            unsigned char c = static_cast<unsigned char>(input[pos++]);
            if (c == '"') { closed = true; break; }
            if (c < 0x20) throw std::runtime_error("control character in string");
            if (c != '\\') { bytes.push_back(static_cast<char>(c)); continue; }
            if (pos == input.size()) throw std::runtime_error("short escape");
            switch (input[pos++]) {
            case '"': bytes.push_back('"'); break; case '\\': bytes.push_back('\\'); break;
            case '/': bytes.push_back('/'); break; case 'b': bytes.push_back('\b'); break;
            case 'f': bytes.push_back('\f'); break; case 'n': bytes.push_back('\n'); break;
            case 'r': bytes.push_back('\r'); break; case 't': bytes.push_back('\t'); break;
            case 'u': {
                unsigned cp = Hex();
                if (cp >= 0xd800 && cp <= 0xdbff) {
                    if (pos + 2 > input.size() || input[pos++] != '\\' || input[pos++] != 'u') throw std::runtime_error("missing low surrogate");
                    unsigned low = Hex();
                    if (low < 0xdc00 || low > 0xdfff) throw std::runtime_error("bad low surrogate");
                    cp = 0x10000 + ((cp - 0xd800) << 10) + low - 0xdc00;
                } else if (cp >= 0xdc00 && cp <= 0xdfff) throw std::runtime_error("unpaired surrogate");
                Utf8(bytes, cp); break;
            }
            default: throw std::runtime_error("invalid escape");
            }
        }
        if (!closed) throw std::runtime_error("unterminated string");
        return std::wstring_convert<std::codecvt_utf8_utf16<wchar_t>>{}.from_bytes(bytes);
    }
    Value Parse(unsigned depth = 0) {
        if (depth > 32) throw std::runtime_error("JSON nesting too deep");
        Space(); if (pos == input.size()) throw std::runtime_error("missing value");
        Value v;
        if (input[pos] == '"') { v.kind = Value::String; v.text = StringValue(); }
        else if (Eat('{')) {
            v.kind = Value::Object;
            if (!Eat('}')) do {
                Space(); if (pos == input.size() || input[pos] != '"') throw std::runtime_error("object key expected");
                auto key = StringValue(); Need(':');
                if (!v.object.emplace(std::move(key), Parse(depth + 1)).second) throw std::runtime_error("duplicate key");
                if (Eat('}')) break;
                Need(',');
            } while (true);
        } else if (Eat('[')) {
            v.kind = Value::Array;
            if (!Eat(']')) do {
                v.array.push_back(Parse(depth + 1));
                if (Eat(']')) break;
                Need(',');
            } while (true);
        } else if (input.compare(pos, 4, "true") == 0 || input.compare(pos, 5, "false") == 0) {
            v.kind = Value::Bool; v.boolean = input[pos] == 't'; pos += v.boolean ? 4 : 5;
        } else if (input.compare(pos, 4, "null") == 0) pos += 4;
        else throw std::runtime_error("unsupported JSON value");
        return v;
    }
public:
    explicit Parser(const std::string& s) : input(s) {}
    Value Root() { auto result = Parse(); Space(); if (pos != input.size()) throw std::runtime_error("trailing data"); return result; }
};
std::wstring Field(const Value& v, const wchar_t* name) {
    auto it = v.object.find(name);
    if (it == v.object.end() || it->second.kind != Value::String || it->second.text.empty()) throw std::runtime_error("missing or invalid string field");
    return it->second.text;
}
}
std::wstring Normalize(const std::wstring& path, const std::wstring& cwd) {
    std::wstring p = path;
    std::replace(p.begin(), p.end(), L'/', L'\\');
    // An NT/extended prefix marks an absolute path. If what remains is not a drive path
    // (e.g. a HID/USB device object), it must never fall through to the relative branch.
    const bool nativePrefix = p.compare(0, 4, L"\\??\\") == 0 || p.compare(0, 4, L"\\\\?\\") == 0;
    if (nativePrefix) p.erase(0, 4);
    if (nativePrefix && !Drive(p)) return {};
    // UNC, device paths and drive-relative paths are intentionally not redirected.
    if (!Drive(p)) {
        if (p.empty() || p[0] == L'\\' || (p.size() >= 2 && p[1] == L':')) return {};
        auto base = Normalize(cwd, L"");
        if (base.empty()) return {};
        p = base + L"\\" + p;
    }
    if (!Drive(p)) return {};
    std::wstring out(1, p[0]); out += L":\\";
    std::vector<std::wstring> parts;
    for (size_t i = 3; i < p.size();) {
        while (i < p.size() && p[i] == L'\\') ++i;
        size_t end = p.find(L'\\', i); if (end == std::wstring::npos) end = p.size();
        if (end == i) break;
        auto part = p.substr(i, end - i);
        if (part == L"..") { if (!parts.empty()) parts.pop_back(); }
        else if (part != L".") {
            if (part.find(L':') != std::wstring::npos) return {}; // no alternate data streams
            parts.push_back(std::move(part));
        }
        i = end;
    }
    for (const auto& part : parts) { if (out.size() > 3) out += L'\\'; out += part; }
    return out;
}
bool Within(const std::wstring& path, const std::wstring& prefix) {
    return !prefix.empty() && path.size() >= prefix.size() &&
        std::equal(prefix.begin(), prefix.end(), path.begin(),
                   [](wchar_t a, wchar_t b) { return Lower(a) == Lower(b); }) &&
        (path.size() == prefix.size() || prefix.back() == L'\\' || path[prefix.size()] == L'\\');
}
std::wstring Rewrite(const std::wstring& path, const std::wstring& cwd, const std::vector<Redirect>& rules, bool* changed) {
    if (changed) *changed = false;
    auto normalized = Normalize(path, cwd);
    if (normalized.empty()) return path;
    for (const auto& rule : rules) {
        if (rule.mode != L"redirect") continue;
        auto from = Normalize(rule.from, L""), to = Normalize(rule.to, L"");
        if (Within(normalized, from) && !to.empty()) {
            if (changed) *changed = true;
            return to + normalized.substr(from.size());
        }
    }
    return path;
}
OverlayDecision DecideOverlay(const std::wstring& path, const std::wstring& cwd,
    const std::vector<Redirect>& rules, bool writing, bool overlayExists, bool originalExists, bool deleted) {
    OverlayDecision result;
    auto normalized = Normalize(path, cwd);
    if (normalized.empty()) return result;
    for (const auto& rule : rules) {
        if (rule.mode != L"overlay") continue;
        auto from = Normalize(rule.from, L""), to = Normalize(rule.to, L"");
        if (from.empty() || to.empty() || !Within(normalized, from) || Within(normalized, to)) continue;
        result.matched = true;
        result.relative = normalized.substr(from.size());
        result.original = normalized;
        result.overlay = to + result.relative;
        result.hidden = deleted && !overlayExists;
        result.useOverlay = writing || overlayExists;
        result.copyUp = writing && originalExists && !overlayExists && !deleted;
        return result;
    }
    return result;
}
std::vector<std::wstring> MergeNames(const std::vector<std::wstring>& original,
    const std::vector<std::wstring>& overlay, const std::vector<std::wstring>& deleted) {
    auto key = [](std::wstring name) {
        std::transform(name.begin(), name.end(), name.begin(), Lower);
        return name;
    };
    std::vector<std::wstring> result;
    std::vector<std::wstring> seen;
    for (const auto& name : deleted) seen.push_back(key(name));
    for (const auto& group : {overlay, original}) for (const auto& name : group) {
        auto folded = key(name);
        if (folded == L".kinseat-deleted" || std::find(seen.begin(), seen.end(), folded) != seen.end()) continue;
        seen.push_back(std::move(folded)); result.push_back(name);
    }
    return result;
}
bool ParseManifest(const std::string& utf8, Manifest& result, std::string& error) {
    try {
        const std::string body = utf8.compare(0, 3, "\xEF\xBB\xBF") == 0 ? utf8.substr(3) : utf8;
        auto root = Parser(body).Root();
        if (root.kind != Value::Object) throw std::runtime_error("manifest must be an object");
        Manifest parsed;
        parsed.gameId = Field(root, L"gameId"); parsed.account = Field(root, L"account");
        parsed.accountSid = Field(root, L"accountSid"); parsed.profileDir = Field(root, L"profileDir");
        parsed.logFile = Field(root, L"logFile");
        auto flag = root.object.find(L"copyFromHostOnce");
        if (flag == root.object.end() || flag->second.kind != Value::Bool) throw std::runtime_error("invalid copyFromHostOnce");
        parsed.copyFromHostOnce = flag->second.boolean;
        auto redirects = root.object.find(L"redirects");
        if (redirects == root.object.end() || redirects->second.kind != Value::Array) throw std::runtime_error("invalid redirects");
        for (const auto& item : redirects->second.array) {
            if (item.kind != Value::Object) throw std::runtime_error("invalid redirect");
            Redirect rule{ Field(item, L"from"), Field(item, L"to"), Field(item, L"mode") };
            if ((rule.mode != L"redirect" && rule.mode != L"overlay") || Normalize(rule.from, L"").empty() || Normalize(rule.to, L"").empty())
                throw std::runtime_error("invalid redirect path or mode");
            parsed.redirects.push_back(std::move(rule));
        }
        if (Normalize(parsed.profileDir, L"").empty() || Normalize(parsed.logFile, L"").empty()) throw std::runtime_error("invalid profile or log path");
        result = std::move(parsed); error.clear(); return true;
    } catch (const std::exception& ex) { error = ex.what(); return false; }
}
}
