using System;
using Freeq.WinUI.Models;
using Freeq.WinUI.Services;
using Freeq.WinUI.ViewModels;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Freeq.WinUI.Controls;

public sealed partial class Sidebar : UserControl
{
    public MainViewModel? ViewModel { get; set; }

    public event Action<ChannelModel>? ChannelSelected;
    public event Action? DisconnectRequested;
    public event Action<string>? JoinRequested;

    public Sidebar()
    {
        InitializeComponent();
    }

    public void Bind(MainViewModel vm)
    {
        ViewModel = vm;
        ChannelsList.ItemsSource = vm.Channels;
        FavoritesList.ItemsSource = vm.FavoriteChannels;
        DmsList.ItemsSource = vm.DirectMessages;

        vm.PropertyChanged += (s, e) =>
        {
            switch (e.PropertyName)
            {
                case nameof(MainViewModel.Nickname):
                    UserNick.Text = vm.Nickname;
                    UserInitial.Text = string.IsNullOrEmpty(vm.Nickname) ? "?" : vm.Nickname[..1].ToUpper();
                    break;
                case nameof(MainViewModel.ConnectionState):
                    UpdateStatusDot(vm.ConnectionState);
                    UserStatus.Text = vm.ConnectionStatus;
                    break;
                case nameof(MainViewModel.ConnectionStatus):
                    UserStatus.Text = vm.ConnectionStatus;
                    break;
            }
        };

        vm.FavoriteChannels.CollectionChanged += (s, e) =>
        {
            FavoritesSection.Visibility = vm.FavoriteChannels.Count > 0
                ? Visibility.Visible : Visibility.Collapsed;
        };
    }

    private void UpdateStatusDot(ConnectionState state)
    {
        StatusDot.Fill = state switch
        {
            ConnectionState.Authenticated => new SolidColorBrush(ColorHelper.FromArgb(255, 0, 212, 170)),
            ConnectionState.Connected or ConnectionState.Connecting or ConnectionState.Authenticating
                => new SolidColorBrush(ColorHelper.FromArgb(255, 255, 181, 71)),
            _ => new SolidColorBrush(ColorHelper.FromArgb(255, 85, 85, 112)),
        };
    }

    private void OnChannelClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is ChannelModel channel)
            ChannelSelected?.Invoke(channel);
    }

    private async void OnJoinChannelClick(object sender, RoutedEventArgs e)
    {
        var dialog = new ContentDialog
        {
            Title = "Join Channel",
            PrimaryButtonText = "Join",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = this.XamlRoot,
        };

        var input = new TextBox
        {
            PlaceholderText = "#channel-name",
            Background = new SolidColorBrush(ColorHelper.FromArgb(255, 28, 28, 36)),
        };
        dialog.Content = input;

        var result = await dialog.ShowAsync();
        if (result == ContentDialogResult.Primary && !string.IsNullOrWhiteSpace(input.Text))
            JoinRequested?.Invoke(input.Text);
    }

    private void OnDisconnectClick(object sender, RoutedEventArgs e)
    {
        DisconnectRequested?.Invoke();
    }

    public static Visibility IntToVisibility(int value)
    {
        return value > 0 ? Visibility.Visible : Visibility.Collapsed;
    }
}
