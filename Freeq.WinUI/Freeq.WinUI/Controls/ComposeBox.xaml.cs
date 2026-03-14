using Freeq.WinUI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace Freeq.WinUI.Controls;

public sealed partial class ComposeBox : UserControl
{
    private MainViewModel? _vm;

    public ComposeBox()
    {
        InitializeComponent();
    }

    public void Bind(MainViewModel vm)
    {
        _vm = vm;
        vm.PropertyChanged += (s, e) =>
        {
            if (e.PropertyName == nameof(MainViewModel.SelectedChannel))
            {
                MessageInput.PlaceholderText = vm.SelectedChannel != null
                    ? $"Message {vm.SelectedChannel.Name}"
                    : "Select a channel...";
            }
        };
    }

    private void OnInputKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Enter && _vm != null)
        {
            _vm.ComposeText = MessageInput.Text;
            _vm.SendMessageCommand.Execute(null);
            MessageInput.Text = "";
            e.Handled = true;
        }
    }

    private void OnSendClick(object sender, RoutedEventArgs e)
    {
        if (_vm == null) return;
        _vm.ComposeText = MessageInput.Text;
        _vm.SendMessageCommand.Execute(null);
        MessageInput.Text = "";
        MessageInput.Focus(FocusState.Programmatic);
    }
}
