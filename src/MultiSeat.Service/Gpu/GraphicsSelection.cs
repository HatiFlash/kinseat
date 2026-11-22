using System.Text.Json;
using MultiSeat.Shared.Models;

namespace MultiSeat.Service.Gpu;

internal readonly record struct GraphicsSelection(GraphicsProfile Profile, int Width, int Height,
    int Fps, NvencQualityPreset Nvenc, int BudgetMb)
{
    internal static bool Valid(JsonElement p, bool allowPartial = false, bool allowNullBudget = false)
    {
        if (!p.TryGetProperty("graphicsProfile", out var element)) return allowPartial &&
            (!p.TryGetProperty("vramBudgetMb", out var budget) || ValidBudget(budget, allowNullBudget));
        if (element.ValueKind != JsonValueKind.String ||
            !Enum.TryParse<GraphicsProfile>(element.GetString(), false, out var profile) ||
            !Enum.IsDefined(profile)) return false;
        var custom = profile == GraphicsProfile.Custom;
        if (custom != (p.TryGetProperty("width", out _)) ||
            custom != p.TryGetProperty("height", out _) ||
            custom != p.TryGetProperty("fps", out _) ||
            custom != p.TryGetProperty("nvencPreset", out _)) return false;
        if (custom && (!IntRange(p.GetProperty("width"), 640, 7680) ||
            !IntRange(p.GetProperty("height"), 480, 4320) ||
            !IntRange(p.GetProperty("fps"), 1, 240) ||
            p.GetProperty("nvencPreset").ValueKind != JsonValueKind.String ||
            !Enum.TryParse<NvencQualityPreset>(p.GetProperty("nvencPreset").GetString(), false, out var nvenc) ||
            !Enum.IsDefined(nvenc))) return false;
        return !p.TryGetProperty("vramBudgetMb", out var vram) || ValidBudget(vram, allowNullBudget);
    }

    internal static bool IntRange(JsonElement element, int min, int max) =>
        element.ValueKind == JsonValueKind.Number && element.TryGetInt32(out var value) && value >= min && value <= max;

    // The app used to serialize an absent optional budget as null on join acceptance.
    private static bool ValidBudget(JsonElement value, bool allowNull) =>
        allowNull && value.ValueKind == JsonValueKind.Null || IntRange(value, 1, 262144);

    internal static GraphicsSelection Parse(JsonElement p)
    {
        var profile = Enum.Parse<GraphicsProfile>(p.GetProperty("graphicsProfile").GetString()!);
        var defaults = profile == GraphicsProfile.Custom ? default : SeatGraphics.Defaults(profile);
        return new GraphicsSelection(profile,
            profile == GraphicsProfile.Custom ? p.GetProperty("width").GetInt32() : defaults.Width,
            profile == GraphicsProfile.Custom ? p.GetProperty("height").GetInt32() : defaults.Height,
            profile == GraphicsProfile.Custom ? p.GetProperty("fps").GetInt32() : defaults.Fps,
            profile == GraphicsProfile.Custom ? Enum.Parse<NvencQualityPreset>(p.GetProperty("nvencPreset").GetString()!) : defaults.Nvenc,
            p.TryGetProperty("vramBudgetMb", out var budget) && budget.ValueKind != JsonValueKind.Null
                ? budget.GetInt32() : defaults.BudgetMb);
    }
}
