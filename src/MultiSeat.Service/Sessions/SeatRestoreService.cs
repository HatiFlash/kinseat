using Microsoft.Extensions.Hosting;

namespace MultiSeat.Service.Sessions;

/// <summary>Resumes saved seats once the worker's startup orphan cleanup has run.</summary>
internal sealed class SeatRestoreService(SeatManager seats, ILogger<SeatRestoreService> logger)
    : BackgroundService
{
    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        try
        {
            await seats.RestoreSeatsAsync(stoppingToken);
            using var timer = new PeriodicTimer(TimeSpan.FromSeconds(5));
            while (await timer.WaitForNextTickAsync(stoppingToken))
                seats.SaveDefinitions();
        }
        catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested) { }
        catch (Exception ex)
        {
            logger.LogError(ex, "Seat restore service stopped unexpectedly");
        }
    }

    public override async Task StopAsync(CancellationToken cancellationToken)
    {
        seats.SaveDefinitions();
        await base.StopAsync(cancellationToken);
    }
}
