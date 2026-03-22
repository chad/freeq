using Freeq.WinUI.Controls;
using Freeq.WinUI.Models;
using Freeq.WinUI.ViewModels;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace Freeq.WinUI;

public sealed partial class MainWindow : Window
{
    private readonly MainViewModel _vm;

    public MainWindow()
    {
        InitializeComponent();

        Title = "Freeq";
        _vm = new MainViewModel(DispatcherQueue);

        SidebarControl.Bind(_vm);
        TopBarControl.Bind(_vm);
        MessageListControl.Bind(_vm);
        ComposeBoxControl.Bind(_vm);
        MemberListControl.Bind(_vm);

        ConnectDialogControl.ConnectRequested += OnConnectRequested;
        SidebarControl.ChannelSelected += OnChannelSelected;
        SidebarControl.DisconnectRequested += OnDisconnectRequested;
        SidebarControl.JoinRequested += OnJoinRequested;
        TopBarControl.MemberListToggled += OnMemberListToggled;

        _vm.PropertyChanged += (s, e) =>
        {
            if (e.PropertyName == nameof(MainViewModel.ShowConnectDialog))
            {
                ConnectDialogControl.Visibility = _vm.ShowConnectDialog
                    ? Visibility.Visible : Visibility.Collapsed;
                MainLayout.Visibility = _vm.ShowConnectDialog
                    ? Visibility.Collapsed : Visibility.Visible;

                if (_vm.ShowConnectDialog)
                    ConnectDialogControl.SetConnecting(false);
            }
            else if (e.PropertyName == nameof(MainViewModel.IsMemberListVisible))
            {
                MemberListControl.Visibility = _vm.IsMemberListVisible
                    ? Visibility.Visible : Visibility.Collapsed;
            }
            else if (e.PropertyName == nameof(MainViewModel.ErrorMessage) && _vm.ErrorMessage != null)
            {
                ConnectDialogControl.ShowError(_vm.ErrorMessage);
                ConnectDialogControl.SetConnecting(false);
            }
        };
    }

    private async void OnConnectRequested(ConnectRequest request)
    {
        _vm.ServerUrl = request.ServerUrl;
        _vm.Nickname = request.Nickname;

        // Set SASL credentials if AT Protocol login
        if (request.IsAtProto && !string.IsNullOrEmpty(request.SaslToken))
        {
            _vm.SetSaslCredentials(
                request.SaslToken,
                request.Did ?? "",
                request.PdsUrl ?? "",
                "web-token"
            );
        }

        // Queue channels BEFORE connecting — they'll be joined when 001 arrives
        if (!string.IsNullOrWhiteSpace(request.Channels))
        {
            var channels = request.Channels.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
            _vm.QueueJoinChannels(channels);
        }

        await _vm.ConnectCommand.ExecuteAsync(null);
    }

    private void OnChannelSelected(ChannelModel channel)
    {
        _vm.SelectedChannel = channel;
    }

    private void OnDisconnectRequested()
    {
        _vm.DisconnectCommand.Execute(null);
    }

    private void OnJoinRequested(string channel)
    {
        _vm.JoinChannelCommand.Execute(channel);
    }

    private void OnMemberListToggled()
    {
        _vm.ToggleMemberListCommand.Execute(null);
    }
}
