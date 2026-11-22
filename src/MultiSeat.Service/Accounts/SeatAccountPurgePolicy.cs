namespace MultiSeat.Service.Accounts;

/// <summary>Fail closed: a seat account may be destroyed only when every identity check succeeded.</summary>
internal static class SeatAccountPurgePolicy
{
    internal const string ManagedDescription = "MultiSeat multi-seat account (managed)";

    internal static string? InitialRefusal(bool inManagedRegistry, string? description, bool isAdministrator) =>
        isAdministrator ? "admin" :
        !inManagedRegistry && !string.Equals(description, ManagedDescription, StringComparison.Ordinal) ? "not_managed" : null;

    internal static bool CanDelete(bool inManagedRegistry, string? description,
        bool isCaller, bool isAdministrator, bool isConsoleAccount) =>
        (inManagedRegistry || string.Equals(description, ManagedDescription, StringComparison.Ordinal)) &&
        !isCaller && !isAdministrator && !isConsoleAccount;
}
