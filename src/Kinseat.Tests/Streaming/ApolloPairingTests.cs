using System.Net;
using System.Text.Json;
using MultiSeat.Service.Streaming;
using Xunit;

namespace MultiSeat.Tests.Streaming;

public class ApolloPairingTests
{
    [Theory]
    [InlineData("0123", "Tablette", true)]
    [InlineData("123", "Tablette", false)]
    [InlineData("１２３４", "Tablette", false)]
    [InlineData("1234", "", false)]
    [InlineData("1234", "a\n", false)]
    public void ValidatesInput(string pin, string name, bool expected) =>
        Assert.Equal(expected, ApolloPairingClient.Valid(pin, name));

    private sealed class Handler(Func<HttpRequestMessage, HttpResponseMessage> respond) : HttpMessageHandler
    {
        public List<string> Paths { get; } = [];
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct)
        {
            Paths.Add(request.RequestUri!.AbsolutePath);
            return Task.FromResult(respond(request));
        }
    }

    private static string Body(HttpRequestMessage r) => r.Content!.ReadAsStringAsync().GetAwaiter().GetResult();

    [Fact]
    public async Task LogsInThenPostsPinToLoopback()
    {
        var handler = new Handler(request =>
        {
            Assert.Equal(HttpMethod.Post, request.Method);
            Assert.Equal("127.0.0.1", request.RequestUri!.Host);
            Assert.Equal(47990, request.RequestUri.Port);
            Assert.Equal("application/json", request.Content!.Headers.ContentType!.MediaType);
            using var body = JsonDocument.Parse(Body(request));
            if (request.RequestUri.AbsolutePath == "/api/login")
            {
                Assert.Equal("kinseat", body.RootElement.GetProperty("username").GetString());
                Assert.Equal("secret", body.RootElement.GetProperty("password").GetString());
                return new HttpResponseMessage(HttpStatusCode.OK);
            }
            Assert.Equal("0123", body.RootElement.GetProperty("pin").GetString());
            Assert.Equal("Tablette", body.RootElement.GetProperty("name").GetString());
            return new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent("{\"status\":true}") };
        });
        using var client = new HttpClient(handler);
        var outcome = await ApolloPairingClient.SubmitAsync(client, 47990, "0123", "Tablette", "kinseat", "secret", default);
        Assert.Equal(ApolloPairingClient.Outcome.Paired, outcome);
        Assert.Equal(["/api/login", "/api/pin"], handler.Paths);
    }

    [Fact]
    public async Task FalseStatusIsAPinRejection()
    {
        using var client = new HttpClient(new Handler(r => r.RequestUri!.AbsolutePath == "/api/login"
            ? new HttpResponseMessage(HttpStatusCode.OK)
            : new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent("{\"status\":false}") }));
        Assert.Equal(ApolloPairingClient.Outcome.PinRejected,
            await ApolloPairingClient.SubmitAsync(client, 47990, "0123", "Tablette", "kinseat", "x", default));
    }

    [Fact]
    public async Task UnauthorizedLoginIsReported()
    {
        var handler = new Handler(_ => new HttpResponseMessage(HttpStatusCode.Unauthorized));
        using var client = new HttpClient(handler);
        Assert.Equal(ApolloPairingClient.Outcome.LoginRejected,
            await ApolloPairingClient.SubmitAsync(client, 47990, "0123", "Tablette", "kinseat", "x", default));
        Assert.Equal(["/api/login"], handler.Paths);
    }

    [Fact]
    public async Task GrantPlayKeepsDeviceSettingsAndAddsInputAndLaunch()
    {
        string? posted = null;
        var handler = new Handler(request =>
        {
            Assert.Equal("127.0.0.1", request.RequestUri!.Host);
            if (request.RequestUri.AbsolutePath == "/api/clients/list")
                return new HttpResponseMessage(HttpStatusCode.OK)
                {
                    Content = new StringContent("""
                        {"status":true,"named_certs":[
                          {"name":"Tablette","uuid":"OLD","perm":50331648},
                          {"name":"Salon","uuid":"S1","perm":50331648},
                          {"name":"Tablette","uuid":"NEW","perm":50331648,"display_mode":"1920x1080x60",
                           "enable_legacy_ordering":false,"allow_client_commands":false,"always_use_virtual_display":true,
                           "do":[{"cmd":"a","elevated":false}],"undo":[{"cmd":"b","elevated":true}]}]}
                        """),
                };
            posted = Body(request);
            return new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent("{\"status\":true}") };
        });
        using var client = new HttpClient(handler);
        Assert.True(await ApolloPairingClient.GrantPlayAsync(client, 47990, "Tablette", default));
        Assert.Equal(["/api/clients/list", "/api/clients/update"], handler.Paths);
        using var body = JsonDocument.Parse(posted!);
        var r = body.RootElement;
        Assert.Equal("NEW", r.GetProperty("uuid").GetString());
        Assert.Equal(50331648u | ApolloPairingClient.PermPlay, r.GetProperty("perm").GetUInt32());
        Assert.Equal(0x7031F00u, ApolloPairingClient.PermPlay);
        Assert.Equal("1920x1080x60", r.GetProperty("display_mode").GetString());
        Assert.False(r.GetProperty("enable_legacy_ordering").GetBoolean());
        Assert.False(r.GetProperty("allow_client_commands").GetBoolean());
        Assert.True(r.GetProperty("always_use_virtual_display").GetBoolean());
        Assert.Equal("a", r.GetProperty("do")[0].GetProperty("cmd").GetString());
        Assert.True(r.GetProperty("undo")[0].GetProperty("elevated").GetBoolean());
    }

    [Fact]
    public async Task GrantPlayFailsWhenDeviceIsUnknownAndSkipsWhenAlreadyGranted()
    {
        var handler = new Handler(_ => new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new StringContent("{\"named_certs\":[{\"name\":\"Salon\",\"uuid\":\"S1\",\"perm\":117645056}]}"),
        });
        using var client = new HttpClient(handler);
        Assert.False(await ApolloPairingClient.GrantPlayAsync(client, 47990, "Tablette", default));
        Assert.True(await ApolloPairingClient.GrantPlayAsync(client, 47990, "Salon", default));
        Assert.Equal(["/api/clients/list", "/api/clients/list"], handler.Paths);
    }

    [Fact]
    public async Task ServerErrorThrows()
    {
        using var client = new HttpClient(new Handler(_ => new HttpResponseMessage(HttpStatusCode.InternalServerError)));
        await Assert.ThrowsAsync<InvalidOperationException>(() =>
            ApolloPairingClient.SubmitAsync(client, 47990, "0123", "Tablette", "kinseat", "x", default));
    }

    [Fact]
    public async Task FirstPairingRetriesRejectedPinUntilClientReady()
    {
        var rejected = 0;
        var handler = new Handler(request => request.RequestUri!.AbsolutePath == "/api/login"
            ? new HttpResponseMessage(HttpStatusCode.OK)
            : new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent(++rejected <= 3 ? "{\"status\":false}" : "{\"status\":true}")
            });
        using var client = new HttpClient(handler);
        var outcome = await ApolloPairingClient.SubmitWithRetryAsync(
            ct => ApolloPairingClient.SubmitAsync(client, 47990, "0123", "Tablette", "kinseat", "secret", ct),
            default, TimeSpan.Zero, TimeSpan.FromSeconds(1));
        Assert.Equal(ApolloPairingClient.Outcome.Paired, outcome);
        Assert.Equal(4, rejected);
    }

    [Fact]
    public async Task LoginRejectionIsNotRetried()
    {
        var calls = 0;
        var outcome = await ApolloPairingClient.SubmitWithRetryAsync(_ =>
        {
            calls++;
            return Task.FromResult(ApolloPairingClient.Outcome.LoginRejected);
        }, default, TimeSpan.Zero, TimeSpan.FromSeconds(1));
        Assert.Equal(ApolloPairingClient.Outcome.LoginRejected, outcome);
        Assert.Equal(1, calls);
    }

    [Fact]
    public void GeneratedPasswordsAreLongAndDistinct()
    {
        var a = ApolloPairingClient.NewPassword();
        Assert.Equal(48, a.Length);
        Assert.NotEqual(a, ApolloPairingClient.NewPassword());
    }
}
