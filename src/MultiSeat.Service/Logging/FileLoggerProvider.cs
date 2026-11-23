using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;
using System.Threading.Channels;

namespace MultiSeat.Service.Logging;

public sealed class FileLoggerProvider : ILoggerProvider
{
    internal const long MaxBytes = 10 * 1024 * 1024;
    private readonly string _directory;
    private readonly Channel<string> _queue = Channel.CreateBounded<string>(new BoundedChannelOptions(2048)
    {
        FullMode = BoundedChannelFullMode.DropWrite, SingleReader = true
    });
    private readonly Task _writer;
    private volatile LogLevel _minimum = LogLevel.Information;

    internal FileLoggerProvider(string? directory = null)
    {
        _directory = directory ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "Kinseat", "logs");
        Directory.CreateDirectory(_directory);
        if (OperatingSystem.IsWindows()) RestrictDirectory(_directory);
        _writer = Task.Run(WriteLoopAsync);
    }

    internal void SetLevel(bool detailed) => _minimum = detailed ? LogLevel.Debug : LogLevel.Information;
    internal bool Detailed => _minimum == LogLevel.Debug;
    public ILogger CreateLogger(string categoryName) => new FileLogger(this, categoryName);

    private sealed class FileLogger(FileLoggerProvider owner, string category) : ILogger
    {
        public IDisposable BeginScope<TState>(TState state) where TState : notnull => NoopScope.Instance;
        private sealed class NoopScope : IDisposable
        {
            internal static readonly NoopScope Instance = new();
            public void Dispose() { }
        }
        public bool IsEnabled(LogLevel level) => level >= owner._minimum && level != LogLevel.None;
        public void Log<TState>(LogLevel level, EventId eventId, TState state, Exception? exception, Func<TState, Exception?, string> formatter)
        {
            if (!IsEnabled(level)) return;
            var text = $"{DateTimeOffset.UtcNow:O} {level} {category} EventId={eventId.Id} {formatter(state, exception)} {exception}";
            owner._queue.Writer.TryWrite(DiagnosticRedactor.Clean(text.Replace('\r', ' ').Replace('\n', ' ')) + Environment.NewLine);
        }
    }

    private async Task WriteLoopAsync()
    {
        DateOnly lastPrunedDay = default;
        await foreach (var line in _queue.Reader.ReadAllAsync())
        {
            try
            {
                var now = DateTime.UtcNow;
                var today = DateOnly.FromDateTime(now);
                var path = Path.Combine(_directory, $"service-{now:yyyyMMdd}.log");
                Rotate(path, Encoding.UTF8.GetByteCount(line));
                await File.AppendAllTextAsync(path, line, Encoding.UTF8);
                if (today != lastPrunedDay)
                {
                    lastPrunedDay = today;
                    foreach (var old in Directory.EnumerateFiles(_directory, "service-*.log*")
                        .Where(f => File.GetLastWriteTimeUtc(f) < now.AddDays(-7))) File.Delete(old);
                }
            }
            catch (Exception) { /* Logging must never block or crash seat provisioning. */ }
        }
    }

    internal static void Rotate(string path, long nextBytes)
    {
        if (!File.Exists(path) || new FileInfo(path).Length + nextBytes <= MaxBytes) return;
        var backup = path + ".1";
        File.Move(path, backup, overwrite: true);
    }

    private static void RestrictDirectory(string path)
    {
        var acl = new DirectorySecurity();
        acl.SetAccessRuleProtection(true, false);
        foreach (var sid in new[] { WellKnownSidType.LocalSystemSid, WellKnownSidType.BuiltinAdministratorsSid })
            acl.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(sid, null),
                FileSystemRights.FullControl, InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit,
                PropagationFlags.None, AccessControlType.Allow));
        new DirectoryInfo(path).SetAccessControl(acl);
    }

    public void Dispose()
    {
        _queue.Writer.TryComplete();
        try { _writer.Wait(TimeSpan.FromSeconds(3)); } catch (Exception) { }
    }
}
