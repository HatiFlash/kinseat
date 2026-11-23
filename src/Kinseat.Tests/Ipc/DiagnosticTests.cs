using System.IO.Compression;
using MultiSeat.Service.Ipc;
using MultiSeat.Service.Logging;
using Xunit;

namespace MultiSeat.Tests.Ipc;

public class DiagnosticTests
{
    [Theory]
    [InlineData("password=bonjour", "bonjour")]
    [InlineData("X-MultiSeat-Key: secret123", "secret123")]
    [InlineData("pin: 1234", "1234")]
    [InlineData(@"C:\Users\joueur\AppData\Local", "joueur")]
    public void RedactsSensitiveText(string input, string secret)
    {
        var result = DiagnosticRedactor.Clean(input);
        Assert.DoesNotContain(secret, result);
        Assert.Contains("[EXPURGÉ]", result);
    }

    [Fact]
    public void RotatesWithoutLosingCurrentLog()
    {
        var dir = Path.Combine(Path.GetTempPath(), Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);
        try
        {
            var path = Path.Combine(dir, "service-20260101.log");
            using (var file = new FileStream(path, FileMode.CreateNew)) file.SetLength(FileLoggerProvider.MaxBytes);
            FileLoggerProvider.Rotate(path, 5);
            Assert.False(File.Exists(path));
            Assert.True(File.Exists(path + ".1"));
        }
        finally { Directory.Delete(dir, true); }
    }

    [Fact]
    public void ArchiveContainsRedactedInputsAndState()
    {
        var root = Path.Combine(Path.GetTempPath(), Guid.NewGuid().ToString("N"));
        try
        {
            var logs = Path.Combine(root, "Kinseat", "logs");
            Directory.CreateDirectory(logs);
            File.WriteAllText(Path.Combine(logs, "service-20260101.log"), "token=unsafe123");
            var agent = Path.Combine(root, "users", "seat0", "AppData", "Local", "Kinseat");
            Directory.CreateDirectory(agent);
            File.WriteAllText(Path.Combine(agent, "agent.log"), "pin=123456");
            var path = DiagnosticExporter.Export(new { status = "ok" }, ["seat0"], root,
                Path.Combine(root, "users"), Path.Combine(root, "out"));
            using var zip = ZipFile.OpenRead(path);
            Assert.Contains(zip.Entries, e => e.FullName == "etat.json");
            Assert.Contains(zip.Entries, e => e.FullName == "agents/place-1/agent.log");
            using var reader = new StreamReader(zip.GetEntry("agents/place-1/agent.log")!.Open());
            Assert.DoesNotContain("123456", reader.ReadToEnd());
        }
        finally { if (Directory.Exists(root)) Directory.Delete(root, true); }
    }
}
