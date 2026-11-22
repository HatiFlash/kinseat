namespace MultiSeat.Shared.Models;

/// <summary>Indicative graphics budget: Windows does not reserve memory per session.</summary>
public enum GraphicsProfile { Economy, Balanced, Quality, Custom }

public static class SeatGraphics
{
    public static (int Width, int Height, int Fps, NvencQualityPreset Nvenc, int BudgetMb) Defaults(GraphicsProfile profile) => profile switch
    {
        GraphicsProfile.Economy => (1280, 720, 60, NvencQualityPreset.Latency, 2048),
        GraphicsProfile.Balanced => (1920, 1080, 60, NvencQualityPreset.Balanced, 3072),
        GraphicsProfile.Quality => (2560, 1440, 60, NvencQualityPreset.Quality, 4096),
        _ => throw new ArgumentException("Le profil personnalisé exige ses paramètres explicites", nameof(profile))
    };

    public static bool Fits(long totalMb, int reserveMb, IEnumerable<int> otherBudgets, int requestedMb) =>
        totalMb > 0 && reserveMb >= 0 && requestedMb > 0 &&
        otherBudgets.All(value => value >= 0) &&
        (long)requestedMb + otherBudgets.Sum(value => (long)value) <= Math.Max(0, totalMb - reserveMb);
}

public sealed class SeatLightening
{
    public bool EmptyDesktop { get; set; } = true;
    public bool DesktopBrowser { get; set; } = false;
    public bool BlockStartupApps { get; set; } = true;
    public bool BlockVendorUtilities { get; set; } = true;
    public bool CloseWindhawkTray { get; set; } = true;
}
