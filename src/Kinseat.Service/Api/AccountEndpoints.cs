using MultiSeat.Service.Accounts;
using MultiSeat.Shared.Models;

namespace MultiSeat.Service.Api;

public static class AccountEndpoints
{
    public static void Map(WebApplication app)
    {
        var group = app.MapGroup("/api/accounts").WithTags("Accounts");

        group.MapGet("/", (AccountManager mgr) =>
            Results.Ok(mgr.ListManagedAccounts()));

        group.MapPost("/", CreateAccount);

        group.MapPost("/link", (AccountCreateRequest request, AccountManager mgr) =>
        {
            if (!ApiInputValidation.IsValidAccountName(request.Username))
                return ApiInputValidation.AccountNameError();
            try
            {
                var account = mgr.LinkExistingAccount(request.Username, request.Password);
                return Results.Created($"/api/accounts/{account.Username}", account);
            }
            catch (InvalidOperationException ex)
            {
                return ApiErrors.ToResult(ex);
            }
        });

        group.MapDelete("/{username}", (string username, AccountManager mgr) =>
        {
            if (!ApiInputValidation.IsValidAccountName(username))
                return ApiInputValidation.AccountNameError();
            try
            {
                mgr.DeleteAccount(username);
                return Results.NoContent();
            }
            catch (InvalidOperationException ex)
            {
                return ApiErrors.ToResult(ex);
            }
        });
    }

    /// <summary>
    /// POST /api/accounts. A method rather than a lambda so tests can call the real handler.
    /// </summary>
    internal static IResult CreateAccount(AccountCreateRequest request, AccountManager mgr)
    {
        if (!ApiInputValidation.IsValidAccountName(request.Username))
            return ApiInputValidation.AccountNameError();

        if (!ApiInputValidation.IsValidCredential(request.Password, request.Credential))
            return ApiInputValidation.PasswordError();

        try
        {
            var account = CreateValidated(request, (name, password) => mgr.CreateAccount(name, password));
            return Results.Created($"/api/accounts/{account.Username}", account);
        }
        catch (InvalidOperationException ex)
        {
            return ApiErrors.ToResult(ex);
        }
    }
    internal static AccountInfo CreateValidated(AccountCreateRequest request,
        Func<string, string?, AccountInfo> create)
    {
        if (!ApiInputValidation.IsValidAccountName(request.Username) ||
            !ApiInputValidation.IsValidCredential(request.Password, request.Credential))
            throw new ArgumentException("Invalid account name or credential.");
        return create(request.Username, request.Credential?.Kind == "none" ? null :
            request.Credential?.Value ?? request.Password);
    }
}

public sealed record AccountCredential(string Kind, string? Value = null);
public sealed record AccountCreateRequest(string Username, string? Password = null, AccountCredential? Credential = null);
