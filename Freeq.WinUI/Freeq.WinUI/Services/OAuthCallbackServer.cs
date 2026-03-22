using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Freeq.WinUI.Services;

internal static class OAuthLog
{
    private static readonly string LogPath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "Freeq", "oauth-debug.log");

    internal static void Write(string msg)
    {
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(LogPath)!);
            File.AppendAllText(LogPath, $"[{DateTime.Now:HH:mm:ss.fff}] {msg}\n");
        }
        catch { /* ignore */ }
        Debug.WriteLine($"[OAuth] {msg}");
    }
}

public class OAuthResult
{
    [JsonPropertyName("did")]
    public string? Did { get; set; }

    [JsonPropertyName("handle")]
    public string? Handle { get; set; }

    [JsonPropertyName("web_token")]
    public string? WebToken { get; set; }

    [JsonPropertyName("token")]
    public string? Token { get; set; }

    [JsonPropertyName("access_jwt")]
    public string? AccessJwt { get; set; }

    [JsonPropertyName("broker_token")]
    public string? BrokerToken { get; set; }

    [JsonPropertyName("pds_url")]
    public string? PdsUrl { get; set; }

    public string EffectiveToken => WebToken ?? Token ?? AccessJwt ?? "";
}

/// <summary>
/// Handles AT Protocol OAuth flow via the freeq auth broker.
/// Based on the working implementation in freeq-windows-app/Auth/BrokerAuth.cs.
/// </summary>
public class OAuthCallbackServer : IDisposable
{
    private const string BrokerBase = "https://auth.freeq.at";
    private static readonly HttpClient Http = new() { Timeout = TimeSpan.FromSeconds(10) };

    private HttpListener? _listener;
    private int _port;

    public string CallbackUrl => $"http://127.0.0.1:{_port}/oauth/callback";

    /// <summary>
    /// Start the OAuth login flow: open browser, listen for callback.
    /// </summary>
    public void StartLogin(string handle)
    {
        _port = FindFreePort();
        OAuthLog.Write($"StartLogin: port={_port}, callback={CallbackUrl}");

        _listener = new HttpListener();
        _listener.Prefixes.Add($"http://127.0.0.1:{_port}/");
        _listener.Start();
        OAuthLog.Write("HttpListener started");

        var authUrl = $"{BrokerBase}/auth/login?handle={Uri.EscapeDataString(handle)}&return_to={Uri.EscapeDataString(CallbackUrl)}";
        OAuthLog.Write($"Opening browser: {authUrl}");
        Process.Start(new ProcessStartInfo(authUrl) { UseShellExecute = true });
    }

    private const string SuccessHtml =
        "<html><body style='background:#0c0c0f;color:#e8e8ed;font-family:Segoe UI,sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0'>" +
        "<div style='text-align:center'><h1 style='color:#00d4aa'>Authenticated!</h1><p>You can close this tab and return to Freeq.</p></div></body></html>";

    private const string ExtractorHtml =
        @"<html><body style='background:#0c0c0f;color:#e8e8ed;font-family:Segoe UI,sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0'>
<div id='msg' style='text-align:center'><h1 style='color:#00d4aa'>freeq</h1><p>Completing authentication...</p></div>
<script>
var h=window.location.hash;
if(h && h.indexOf('oauth=')!==-1){
  var p=h.split('oauth=')[1];
  window.location.replace('/oauth/complete?oauth='+encodeURIComponent(p));
} else {
  document.getElementById('msg').innerHTML='<h1 style=""color:#ff5c5c"">Authentication failed</h1><p>No OAuth data received. Hash: '+h+'</p>';
}
</script></body></html>";

    /// <summary>
    /// Wait for the OAuth callback redirect. Returns the parsed result, or null on timeout.
    /// Runs entirely on a background thread to avoid WinUI DispatcherQueue deadlocks.
    /// </summary>
    public Task<OAuthResult?> WaitForCallbackAsync(TimeSpan timeout)
    {
        if (_listener == null) return Task.FromResult<OAuthResult?>(null);

        // Run the listener loop on a thread pool thread to avoid
        // synchronization context deadlocks with WinUI's DispatcherQueue.
        return Task.Run(() => ListenForOAuthAsync(timeout));
    }

    private async Task<OAuthResult?> ListenForOAuthAsync(TimeSpan timeout)
    {
        OAuthLog.Write("ListenForOAuthAsync started");
        try
        {
            using var cts = new CancellationTokenSource(timeout);
            var servedExtractor = false;

            while (!cts.Token.IsCancellationRequested)
            {
                OAuthLog.Write("Waiting for request...");
                var ctx = await _listener!.GetContextAsync().WaitAsync(cts.Token).ConfigureAwait(false);
                var path = ctx.Request.Url?.AbsolutePath ?? "";
                var rawUrl = ctx.Request.RawUrl ?? "";
                OAuthLog.Write($"Request received: method={ctx.Request.HttpMethod} path={path} rawUrl={rawUrl}");

                // Check for oauth data in query params on any request
                var oauthParam = ctx.Request.QueryString["oauth"];
                OAuthLog.Write($"oauth param present: {!string.IsNullOrEmpty(oauthParam)}, length: {oauthParam?.Length ?? 0}");

                if (!string.IsNullOrEmpty(oauthParam))
                {
                    var result = DecodeOAuthPayload(oauthParam);
                    OAuthLog.Write($"DecodeOAuthPayload: Did={result?.Did}, Token={result?.EffectiveToken?.Length ?? 0} chars");

                    if (result?.Did != null)
                    {
                        // Full page navigation from JS lands here — serve success page
                        await SendHtml(ctx.Response, SuccessHtml).ConfigureAwait(false);
                        OAuthLog.Write("SUCCESS — returning result");
                        return result;
                    }
                    else
                    {
                        OAuthLog.Write("DecodeOAuthPayload returned null Did — trying raw decode for debugging");
                        try
                        {
                            var raw = oauthParam.Replace('-', '+').Replace('_', '/');
                            switch (raw.Length % 4) { case 2: raw += "=="; break; case 3: raw += "="; break; }
                            var json = Encoding.UTF8.GetString(Convert.FromBase64String(raw));
                            OAuthLog.Write($"Raw JSON: {json[..Math.Min(json.Length, 200)]}");
                        }
                        catch (Exception ex) { OAuthLog.Write($"Raw decode failed: {ex.Message}"); }
                    }
                }

                if (path.StartsWith("/oauth/callback", StringComparison.OrdinalIgnoreCase) && !servedExtractor)
                {
                    OAuthLog.Write("Serving extractor page");
                    await SendHtml(ctx.Response, ExtractorHtml).ConfigureAwait(false);
                    servedExtractor = true;
                }
                else
                {
                    OAuthLog.Write($"Spurious request, sending 204: {path}");
                    ctx.Response.StatusCode = 204;
                    ctx.Response.Close();
                }
            }

            OAuthLog.Write("Loop exited — timeout");
            return null;
        }
        catch (OperationCanceledException)
        {
            OAuthLog.Write("OperationCanceledException — timeout");
            return null;
        }
        catch (Exception ex)
        {
            OAuthLog.Write($"Unexpected exception: {ex}");
            return null;
        }
        finally
        {
            OAuthLog.Write("StopListener");
            StopListener();
        }
    }

    public static async Task<bool> HealthCheckAsync()
    {
        try
        {
            var resp = await Http.GetAsync($"{BrokerBase}/health");
            return resp.IsSuccessStatusCode;
        }
        catch { return false; }
    }

    public void Dispose()
    {
        StopListener();
        GC.SuppressFinalize(this);
    }

    private void StopListener()
    {
        try { _listener?.Stop(); } catch { /* ignore */ }
        _listener = null;
    }

    private static async Task SendHtml(HttpListenerResponse response, string html)
    {
        var buffer = Encoding.UTF8.GetBytes(html);
        response.ContentType = "text/html; charset=utf-8";
        response.ContentLength64 = buffer.Length;
        await response.OutputStream.WriteAsync(buffer);
        response.Close();
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
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        var port = ((IPEndPoint)listener.LocalEndpoint).Port;
        listener.Stop();
        return port;
    }
}
