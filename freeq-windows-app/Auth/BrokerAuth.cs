using System.Diagnostics;
using System.Net;
using System.Net.Http;
using System.Text;
using System.Text.Json;

namespace Freeq.Windows.Auth;

/// <summary>
/// Handles AT Protocol OAuth flow via the freeq auth broker.
/// </summary>
public class BrokerAuth
{
    private static readonly string[] DefaultSuffixes =
    [
        ".bsky.social",
        ".bsky.app",
        ".bsky.team",
        ".bsky.network",
        ".atproto.com",
    ];

    private static readonly HttpClient Http = new() { Timeout = TimeSpan.FromSeconds(10) };

    private HttpListener? _listener;

    /// <summary>
    /// Derive an IRC nick from an AT Protocol handle.
    /// Custom domains use the full handle; default hosting strips the suffix.
    /// </summary>
    public static string NickFromHandle(string handle)
    {
        var h = handle.ToLowerInvariant().Trim();
        foreach (var suffix in DefaultSuffixes)
        {
            if (h.EndsWith(suffix))
                return h[..^suffix.Length];
        }
        return h;
    }

    /// <summary>
    /// Start the OAuth login flow by opening the system browser.
    /// Returns a localhost callback URL that will receive the redirect.
    /// </summary>
    public string StartLogin(string handle, string brokerBase)
    {
        // Find an ephemeral port for the callback listener
        var port = FindFreePort();
        var callbackUrl = $"http://127.0.0.1:{port}/oauth/callback";

        // Start listening before opening browser
        _listener = new HttpListener();
        _listener.Prefixes.Add($"http://127.0.0.1:{port}/");
        _listener.Start();

        var authUrl = $"{brokerBase}/auth/login?handle={Uri.EscapeDataString(handle)}&return_to={Uri.EscapeDataString(callbackUrl)}";
        Process.Start(new ProcessStartInfo(authUrl) { UseShellExecute = true });

        return callbackUrl;
    }

    /// <summary>
    /// Wait for the OAuth callback redirect. Returns the parsed result, or null on timeout/error.
    /// </summary>
    public async Task<OAuthResult?> WaitForCallbackAsync(TimeSpan timeout)
    {
        if (_listener == null) return null;

        try
        {
            using var cts = new CancellationTokenSource(timeout);
            var ctx = await _listener.GetContextAsync().WaitAsync(cts.Token);

            // Read the request URL — the broker redirects with #oauth=... in the fragment.
            // Since fragments aren't sent to servers, the broker puts it in a query param
            // or we serve a page that extracts it. Let's check both patterns.
            var requestUrl = ctx.Request.Url?.ToString() ?? "";
            var query = ctx.Request.QueryString;

            OAuthResult? result = null;

            // Check for ?oauth= query parameter (broker may include it)
            var oauthParam = query["oauth"];
            if (!string.IsNullOrEmpty(oauthParam))
            {
                result = DecodeOAuthPayload(oauthParam);
            }

            if (result?.Did != null)
            {
                // Success — send a nice response page
                var html = "<html><body style='background:#1e1e2e;color:#cdd6f4;font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0'><div style='text-align:center'><h1>Authenticated!</h1><p>You can close this tab and return to Freeq.</p></div></body></html>";
                var buffer = Encoding.UTF8.GetBytes(html);
                ctx.Response.ContentType = "text/html";
                ctx.Response.ContentLength64 = buffer.Length;
                await ctx.Response.OutputStream.WriteAsync(buffer);
                ctx.Response.Close();
                return result;
            }

            // No direct query param — serve a page that reads the fragment and posts it back
            var extractorHtml = @"<html><body style='background:#1e1e2e;color:#cdd6f4;font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0'>
<div id='msg' style='text-align:center'><h1>Completing authentication...</h1></div>
<script>
var h=window.location.hash;
if(h.startsWith('#oauth=')){
  var p=h.slice(7);
  fetch('/oauth/complete?oauth='+encodeURIComponent(p)).then(function(){
    document.getElementById('msg').innerHTML='<h1>Authenticated!</h1><p>You can close this tab and return to Freeq.</p>';
  });
} else {
  document.getElementById('msg').innerHTML='<h1>Authentication failed</h1><p>No OAuth data received.</p>';
}
</script></body></html>";
            var buf = Encoding.UTF8.GetBytes(extractorHtml);
            ctx.Response.ContentType = "text/html";
            ctx.Response.ContentLength64 = buf.Length;
            await ctx.Response.OutputStream.WriteAsync(buf);
            ctx.Response.Close();

            // Wait for the second request with the fragment data
            var ctx2 = await _listener.GetContextAsync().WaitAsync(cts.Token);
            var oauthParam2 = ctx2.Request.QueryString["oauth"];
            if (!string.IsNullOrEmpty(oauthParam2))
            {
                result = DecodeOAuthPayload(oauthParam2);
            }

            var successHtml = result?.Did != null ? "OK" : "Failed";
            var buf2 = Encoding.UTF8.GetBytes(successHtml);
            ctx2.Response.ContentLength64 = buf2.Length;
            await ctx2.Response.OutputStream.WriteAsync(buf2);
            ctx2.Response.Close();

            return result;
        }
        catch (OperationCanceledException)
        {
            return null;
        }
        finally
        {
            StopListener();
        }
    }

    /// <summary>
    /// Refresh a persistent broker token to get a fresh web token.
    /// Retries once on 502 (DPoP nonce rotation).
    /// </summary>
    public static async Task<BrokerSessionResponse?> RefreshSessionAsync(string brokerBase, string brokerToken)
    {
        var body = JsonSerializer.Serialize(new { broker_token = brokerToken });
        var content = new StringContent(body, Encoding.UTF8, "application/json");

        try
        {
            var resp = await Http.PostAsync($"{brokerBase}/session", content);

            // Retry once on 502 (DPoP nonce rotation)
            if (resp.StatusCode == HttpStatusCode.BadGateway)
            {
                var content2 = new StringContent(body, Encoding.UTF8, "application/json");
                resp = await Http.PostAsync($"{brokerBase}/session", content2);
            }

            if (!resp.IsSuccessStatusCode) return null;

            var json = await resp.Content.ReadAsStringAsync();
            return JsonSerializer.Deserialize<BrokerSessionResponse>(json);
        }
        catch
        {
            return null;
        }
    }

    /// <summary>
    /// Check if the broker is reachable.
    /// </summary>
    public static async Task<bool> HealthCheckAsync(string brokerBase)
    {
        try
        {
            var resp = await Http.GetAsync($"{brokerBase}/health");
            return resp.IsSuccessStatusCode;
        }
        catch
        {
            return false;
        }
    }

    public void Cancel()
    {
        StopListener();
    }

    private void StopListener()
    {
        try { _listener?.Stop(); } catch { /* ignore */ }
        _listener = null;
    }

    private static OAuthResult? DecodeOAuthPayload(string base64UrlPayload)
    {
        try
        {
            // base64url → base64
            var base64 = base64UrlPayload.Replace('-', '+').Replace('_', '/');
            switch (base64.Length % 4)
            {
                case 2: base64 += "=="; break;
                case 3: base64 += "="; break;
            }
            var bytes = Convert.FromBase64String(base64);
            var json = Encoding.UTF8.GetString(bytes);
            return JsonSerializer.Deserialize<OAuthResult>(json);
        }
        catch
        {
            return null;
        }
    }

    private static int FindFreePort()
    {
        var listener = new System.Net.Sockets.TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        var port = ((IPEndPoint)listener.LocalEndpoint).Port;
        listener.Stop();
        return port;
    }
}
