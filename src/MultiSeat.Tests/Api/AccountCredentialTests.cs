using System.Text.Json;
using MultiSeat.Service.Api;
using Xunit;

namespace MultiSeat.Tests.Api;

public class AccountCredentialTests
{
    [Theory]
    [InlineData("{\"credential\":{\"kind\":\"pin\",\"value\":\"0123\"}}", "0123")]
    [InlineData("{\"credential\":{\"kind\":\"password\",\"value\":\"abcdefgh\"}}", "abcdefgh")]
    [InlineData("{\"credential\":{\"kind\":\"none\"}}", null)]
    [InlineData("{\"password\":\"old\"}", "old")]
    public void AcceptedCredential_IsPassedToAccountCreation(string json, string? expected)
    {
        using var document = JsonDocument.Parse(json);
        Assert.True(ApiInputValidation.TryParseCredential(document.RootElement, out var credential));
        var password = document.RootElement.TryGetProperty("password", out var legacy) ? legacy.GetString() : null;
        string? received = "not-called";
        var request = new AccountCreateRequest("player", password, credential);
        AccountEndpoints.CreateValidated(request, (name, value) =>
        {
            Assert.Equal("player", name);
            received = value;
            return null!;
        });
        Assert.Equal(expected, received);
    }

    [Theory]
    [InlineData("{}")]
    [InlineData("{\"password\":null}")]
    [InlineData("{\"credential\":null}")]
    [InlineData("{\"credential\":{\"kind\":\"pin\",\"value\":\"123\"}}")]
    [InlineData("{\"credential\":{\"kind\":\"pin\",\"value\":\"12a4\"}}")]
    [InlineData("{\"credential\":{\"kind\":\"password\",\"value\":\"1234567\"}}")]
    [InlineData("{\"credential\":{\"kind\":\"none\",\"value\":\"ignored\"}}")]
    [InlineData("{\"credential\":{\"kind\":\"other\"}}")]
    [InlineData("{\"credential\":{\"kind\":\"none\"},\"password\":\"old\"}")]
    [InlineData("{\"credential\":{\"kind\":\"none\",\"kind\":\"none\"}}")]
    [InlineData("{\"credential\":{\"kind\":\"none\",\"extra\":1}}")]
    public void InvalidCredential_IsRejected(string json)
    {
        using var document = JsonDocument.Parse(json);
        Assert.False(ApiInputValidation.TryParseCredential(document.RootElement, out _));
    }
}
