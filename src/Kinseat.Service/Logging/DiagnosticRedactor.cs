using System.Text.RegularExpressions;

namespace MultiSeat.Service.Logging;

/// <summary>Fail-closed sanitisation of text leaving the service for diagnostic files.</summary>
internal static partial class DiagnosticRedactor
{
    [GeneratedRegex("""(?i)("?(?:api[_-]?key|password|passwd|pin|token|authorization|X-MultiSeat-Key|secret)"?\s*[:=]\s*)(?:"(?:\\.|[^"])*"|[^\s,;}&]+)""")]
    private static partial Regex Secrets();
    [GeneratedRegex("""(?i)([?&]key=)[^\s&"'<>]+""")]
    private static partial Regex QueryKeys();
    [GeneratedRegex("""(?i)(?:C:\\Users\\|C:/Users/)[^\\/\s"'<>:]+(?:[\\/][^\s"'<>]*)?""")]
    private static partial Regex Profiles();
    [GeneratedRegex(@"(?i)(Bearer\s+)\S+")]
    private static partial Regex Bearer();

    internal static string Clean(string text) => Profiles().Replace(
        Bearer().Replace(QueryKeys().Replace(Secrets().Replace(text, "$1[EXPURGÉ]"), "$1[EXPURGÉ]"), "$1[EXPURGÉ]"), "[EXPURGÉ]");
}
