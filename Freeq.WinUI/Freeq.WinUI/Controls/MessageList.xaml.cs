using Freeq.WinUI.Models;
using Freeq.WinUI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Freeq.WinUI.Controls;

public sealed partial class MessageList : UserControl
{
    public MessageList()
    {
        InitializeComponent();
    }

    public void Bind(MainViewModel vm)
    {
        MessagesListView.ItemsSource = vm.Messages;

        vm.Messages.CollectionChanged += (s, e) =>
        {
            EmptyState.Visibility = vm.Messages.Count == 0 ? Visibility.Visible : Visibility.Collapsed;

            // Auto-scroll to bottom
            if (vm.Messages.Count > 0)
            {
                MessagesListView.ScrollIntoView(vm.Messages[^1]);
            }
        };

        vm.PropertyChanged += (s, e) =>
        {
            if (e.PropertyName == nameof(MainViewModel.SelectedChannel))
            {
                EmptyState.Visibility = vm.Messages.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
                EmptyChannelName.Text = vm.SelectedChannel?.DisplayName ?? "";
            }
        };
    }

    public static Visibility BoolToVis(bool value) =>
        value ? Visibility.Visible : Visibility.Collapsed;
}
