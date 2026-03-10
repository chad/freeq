using System;
using Freeq.WinUI.Services;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.System;

namespace Freeq.WinUI.Controls;

public sealed partial class ConnectDialog : UserControl
{

    private bool _isAtProtoMode = true;
    private bool _isConnecting;
    private bool _advancedVisible;
    private bool _nickManuallyEdited;
    private OAuthCallbackServer? _oauthServer;

    public event Action<ConnectRequest>? ConnectRequested;

    public ConnectDialog()
    {
        InitializeComponent();
        var rng = new Random();
        GuestNickInput.Text = $"desktop{rng.Next(10000, 99999)}";
        SetMode(true);
    }

    public void ShowError(string message)
    {
        ErrorText.Text = message;
        ErrorBox.Visibility = Visibility.Visible;
    }

    public void ClearError()
    {
        ErrorBox.Visibility = Visibility.Collapsed;
    }

    public void SetConnecting(bool connecting)
    {
        _isConnecting = connecting;
        ConnectButton.IsEnabled = !connecting;
        ConnectSpinner.IsActive = connecting;
        ConnectSpinner.Visibility = connecting ? Visibility.Visible : Visibility.Collapsed;
        ConnectButtonText.Text = connecting
            ? (_isAtProtoMode ? "Waiting for authorization..." : "Connecting...")
            : (_isAtProtoMode ? "Sign in with AT Protocol" : "Connect as Guest");
    }

    private void SetMode(bool atProto)
    {
        _isAtProtoMode = atProto;

        AtProtoTab.Background = atProto
            ? new SolidColorBrush(ColorHelper.FromArgb(25, 0, 212, 170))
            : new SolidColorBrush(Colors.Transparent);
        AtProtoTab.Foreground = atProto
            ? new SolidColorBrush(ColorHelper.FromArgb(255, 0, 212, 170))
            : new SolidColorBrush(ColorHelper.FromArgb(255, 85, 85, 112));

        GuestTab.Background = !atProto
            ? new SolidColorBrush(ColorHelper.FromArgb(25, 0, 212, 170))
            : new SolidColorBrush(Colors.Transparent);
        GuestTab.Foreground = !atProto
            ? new SolidColorBrush(ColorHelper.FromArgb(255, 0, 212, 170))
            : new SolidColorBrush(ColorHelper.FromArgb(255, 85, 85, 112));

        AtProtoFields.Visibility = atProto ? Visibility.Visible : Visibility.Collapsed;
        GuestFields.Visibility = atProto ? Visibility.Collapsed : Visibility.Visible;

        ConnectButtonText.Text = atProto ? "Sign in with AT Protocol" : "Connect as Guest";

        if (atProto)
            HandleInput.Focus(FocusState.Programmatic);
        else
            GuestNickInput.Focus(FocusState.Programmatic);
    }

    private void OnAtProtoTabClick(object sender, RoutedEventArgs e) => SetMode(true);
    private void OnGuestTabClick(object sender, RoutedEventArgs e) => SetMode(false);

    private void OnHandleChanged(object sender, TextChangedEventArgs e)
    {
        if (_nickManuallyEdited) return;

        var handle = HandleInput.Text.Trim();
        if (string.IsNullOrEmpty(handle))
        {
            AtNickInput.Text = "";
            return;
        }

        var nick = handle;
        foreach (var suffix in new[] { ".bsky.social", ".bsky.network", ".bsky.app" })
        {
            if (nick.EndsWith(suffix, StringComparison.OrdinalIgnoreCase))
            {
                nick = nick[..^suffix.Length];
                break;
            }
        }

        nick = nick.Replace(".", "").Replace("-", "").Replace("_", "");
        if (nick.Length > 16) nick = nick[..16];

        AtNickInput.Text = nick;
    }

    private void OnAdvancedToggle(object sender, RoutedEventArgs e)
    {
        _advancedVisible = !_advancedVisible;
        AdvancedFields.Visibility = _advancedVisible ? Visibility.Visible : Visibility.Collapsed;
        AdvancedArrow.Text = _advancedVisible ? "\uE70D" : "\uE76C";
    }

    private void OnFieldKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Enter)
        {
            OnConnectClick(sender, e);
            e.Handled = true;
        }
    }

    private async void OnConnectClick(object sender, RoutedEventArgs e)
    {
        if (_isConnecting) return;

        ClearError();
        var server = ServerInput.Text.Trim();
        var channels = ChannelInput.Text.Trim();

        if (_isAtProtoMode)
        {
            var handle = HandleInput.Text.Trim();
            if (string.IsNullOrEmpty(handle))
            {
                ShowError("Please enter your AT Protocol handle.");
                return;
            }

            var nick = string.IsNullOrWhiteSpace(AtNickInput.Text)
                ? handle.Split('.')[0]
                : AtNickInput.Text.Trim();

            SetConnecting(true);

            try
            {
                // Check broker health
                if (!await OAuthCallbackServer.HealthCheckAsync())
                {
                    ShowError("Authentication service is unavailable. Try guest mode.");
                    SetConnecting(false);
                    return;
                }

                // Start local OAuth callback server and open browser
                _oauthServer?.Dispose();
                _oauthServer = new OAuthCallbackServer();
                _oauthServer.StartLogin(handle);

                // Wait for OAuth result (5 minute timeout)
                var oauthResult = await _oauthServer.WaitForCallbackAsync(TimeSpan.FromMinutes(5));
                _oauthServer.Dispose();
                _oauthServer = null;

                if (oauthResult == null)
                {
                    ShowError("Authentication timed out. Please try again.");
                    SetConnecting(false);
                    return;
                }

                var token = oauthResult.EffectiveToken;
                Services.OAuthLog.Write($"OAuth complete: Did={oauthResult.Did}, token={token?.Length ?? 0} chars, PdsUrl={oauthResult.PdsUrl}");

                if (string.IsNullOrEmpty(token))
                {
                    ShowError("No authentication token received. Please try again.");
                    SetConnecting(false);
                    return;
                }

                Services.OAuthLog.Write("Invoking ConnectRequested event");
                // Connect with SASL credentials
                ConnectRequested?.Invoke(new ConnectRequest
                {
                    ServerUrl = server,
                    Nickname = nick,
                    Channels = channels,
                    IsAtProto = true,
                    Handle = handle,
                    SaslToken = token,
                    Did = oauthResult.Did ?? "",
                    PdsUrl = oauthResult.PdsUrl ?? "",
                });
            }
            catch (Exception ex)
            {
                ShowError($"Authentication failed: {ex.Message}");
                SetConnecting(false);
                _oauthServer?.Dispose();
                _oauthServer = null;
            }
        }
        else
        {
            // Guest mode
            var nick = GuestNickInput.Text.Trim();
            if (string.IsNullOrEmpty(nick))
            {
                ShowError("Please enter a nickname.");
                return;
            }

            SetConnecting(true);
            ConnectRequested?.Invoke(new ConnectRequest
            {
                ServerUrl = server,
                Nickname = nick,
                Channels = channels,
                IsAtProto = false,
            });
        }
    }
}

public class ConnectRequest
{
    public string ServerUrl { get; set; } = "";
    public string Nickname { get; set; } = "";
    public string Channels { get; set; } = "#freeq";
    public bool IsAtProto { get; set; }
    public string? Handle { get; set; }
    public string? SaslToken { get; set; }
    public string? Did { get; set; }
    public string? PdsUrl { get; set; }
}
