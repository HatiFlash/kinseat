#include "../GameFilesPaths.h"
#include <iostream>
using namespace game_files;
int main() {
    int errors = 0;
    auto check = [&](bool ok, const char* label) { if (!ok) { std::cerr << label << '\n'; ++errors; } };
    check(Normalize(L"\\??\\C:\\Users\\Public\\Documents\\..\\Documents\\SAVE", L"") == L"C:\\Users\\Public\\Documents\\SAVE", "NT path preserves case");
    check(Normalize(L"\\\\?\\D:/Games/../Saves", L"") == L"D:\\Saves", "extended path");
    check(Normalize(L"../Steam\\SAVE", L"C:\\Users\\Public\\Documents") == L"C:\\Users\\Public\\Steam\\SAVE", "relative path");
    check(Normalize(L"C:relative", L"").empty(), "drive-relative rejected");
    check(Normalize(L"\\??\\HID#VID_045E&PID_028E&IG_01#3&5d59ec1&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}", L"C:\\Games\\Zelda").empty(), "NT device path never relative");
    check(Normalize(L"\\??\\USB#VID_045E&PID_028E#01#{ec87f1e3-c13b-4100-b5f7-8b84d54260cb}", L"C:\\Games\\Zelda").empty(), "NT USB device path never relative");
    check(Normalize(L"\\\\?\\HID#VID_045E&PID_028E#x", L"C:\\Games\\Zelda").empty(), "extended device path never relative");
    check(Normalize(L"save\\a.sav", L"C:\\Games\\Zelda") == L"C:\\Games\\Zelda\\save\\a.sav", "plain relative still resolved");
    check(Normalize(L"\\\\server\\share", L"").empty(), "UNC rejected");
    check(!Within(L"c:\\users\\public\\documents2", L"c:\\users\\public\\documents"), "prefix boundary");
    check(Within(L"c:\\users\\public\\documents\\x", L"c:\\users\\public\\documents"), "prefix match");
    bool changed = false;
    std::vector<Redirect> rules{{L"C:\\Users\\Public\\Documents", L"D:\\Seat\\Saves", L"redirect"}};
    check(Rewrite(L"\\\\?\\C:\\USERS\\Public\\Documents\\Steam\\x", L"", rules, &changed) == L"D:\\Seat\\Saves\\Steam\\x" && changed, "rewrite case preserved");
    check(Rewrite(L"C:\\Users\\Public\\Documents2\\x", L"", rules, &changed) == L"C:\\Users\\Public\\Documents2\\x" && !changed, "no partial rewrite");
    const std::string json = R"({"gameId":"emu","account":"player","accountSid":"S-1-5-21-42","profileDir":"C:\\Users\\player","redirects":[{"from":"C:\\Users\\Public\\Documents","to":"D:\\Seat\\Saves","mode":"redirect"},{"from":"D:\\Game","to":"D:\\Seat\\Game","mode":"overlay"}],"copyFromHostOnce":false,"logFile":"C:\\Users\\player\\log.txt"})";
    Manifest parsed; std::string error;
    check(ParseManifest(json, parsed, error) && parsed.redirects.size() == 2 && parsed.accountSid == L"S-1-5-21-42", "manifest parse");
    check(!ParseManifest(json + "garbage", parsed, error), "trailing data rejected");
    check(!ParseManifest(R"({"gameId":"x","gameId":"y"})", parsed, error), "duplicate rejected");
    check(!ParseManifest(R"({"gameId":"\uD800"})", parsed, error), "surrogate rejected");
    std::vector<Redirect> overlays{{L"D:\\Game", L"E:\\Player\\Game", L"overlay"}};
    auto read = DecideOverlay(L"\\\\?\\d:\\GAME\\Cache.BIN", L"", overlays, false, false, true, false);
    check(read.matched && !read.useOverlay && read.original == L"d:\\GAME\\Cache.BIN" &&
          read.overlay == L"E:\\Player\\Game\\Cache.BIN", "overlay original read");
    auto write = DecideOverlay(L"D:\\Game\\Cache.BIN", L"", overlays, true, false, true, false);
    check(write.matched && write.useOverlay && write.copyUp, "overlay copy up");
    check(!DecideOverlay(L"D:\\Game2\\Cache.BIN", L"", overlays, true, false, true, false).matched, "overlay boundary");
    check(!DecideOverlay(L"\\??\\HID#VID_045E&PID_028E&IG_01#3&5d59ec1&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}", L"D:\\Game", overlays, false, false, false, false).matched, "device path not overlaid from game dir");
    check(DecideOverlay(L"D:\\Game\\Cache.BIN", L"", overlays, false, false, true, true).hidden, "deleted original hidden");
    check(!DecideOverlay(L"D:\\Game\\Cache.BIN", L"", overlays, true, false, true, true).copyUp, "deleted not copied up");
    check(DecideOverlay(L"D:\\Game\\new.txt", L"", overlays, true, false, false, false).useOverlay, "new file overlay");
    check(DecideOverlay(L"D:\\Game\\cache.bin", L"", overlays, false, true, true, false).useOverlay, "overlay read precedence");
    auto merged = MergeNames({L"cache.bin", L"Save.DAT", L"Host.txt"},
                             {L"CACHE.BIN", L"player.txt"}, {L"save.dat"});
    check(merged == std::vector<std::wstring>({L"CACHE.BIN", L"player.txt", L"Host.txt"}), "overlay union");
    auto recreated = MergeNames({L"save.dat"}, {L"save.dat"}, {L"save.dat"});
    check(recreated == std::vector<std::wstring>({L"save.dat"}), "deleted-then-recreated overlay file stays visible");
    return errors ? 1 : 0;
}
