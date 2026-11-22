using System;
using System.IO;
using MultiSeat.Service.Ipc;
using Xunit;

namespace MultiSeat.Tests.Ipc;

// Point 32 : une copie de tâche périmée est archivée, jamais supprimée.
public sealed class LogonTaskManagerArchiveTests : IDisposable
{
    private readonly string _dir = Path.Combine(Path.GetTempPath(), "ks-p32-" + Guid.NewGuid().ToString("N"));

    public LogonTaskManagerArchiveTests() => Directory.CreateDirectory(_dir);

    public void Dispose()
    {
        if (Directory.Exists(_dir)) Directory.Delete(_dir, true);
    }

    [Fact]
    public void ArchiveRenamesBackupAndKeepsContent()
    {
        var backup = Path.Combine(_dir, "task.xml");
        File.WriteAllText(backup, "<task>original</task>");
        var now = new DateTimeOffset(2026, 11, 22, 10, 0, 0, TimeSpan.Zero);

        var archived = LogonTaskManager.ArchiveStaleBackup(backup, now);

        Assert.False(File.Exists(backup));
        Assert.Equal(backup + ".old-20261122T100000Z", archived);
        Assert.Equal("<task>original</task>", File.ReadAllText(archived));
    }

    [Fact]
    public void ArchiveAtSameTimestampKeepsBothCopies()
    {
        var backup = Path.Combine(_dir, "task.xml");
        var now = new DateTimeOffset(2026, 11, 22, 10, 0, 0, TimeSpan.Zero);
        File.WriteAllText(backup, "first");
        var first = LogonTaskManager.ArchiveStaleBackup(backup, now);
        File.WriteAllText(backup, "second");
        var second = LogonTaskManager.ArchiveStaleBackup(backup, now);

        Assert.NotEqual(first, second);
        Assert.Equal(backup + ".old-20261122T100000Z-1", second);
        Assert.Equal("first", File.ReadAllText(first));
        Assert.Equal("second", File.ReadAllText(second));
        Assert.False(File.Exists(backup));
    }
}
