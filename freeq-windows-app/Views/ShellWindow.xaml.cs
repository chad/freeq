using System.Collections.Specialized;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using Freeq.Windows.Models;
using Freeq.Windows.ViewModels;

namespace Freeq.Windows.Views;

public partial class ShellWindow : Window
{
    private ShellViewModel ViewModel => (ShellViewModel)DataContext;

    public ShellWindow()
    {
        InitializeComponent();
        Loaded += OnLoaded;
        Closing += OnClosing;
    }

    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        // Restore window bounds
        var (left, top, width, height) = ViewModel.GetSavedWindowBounds();
        if (width.HasValue && height.HasValue)
        {
            Left = left ?? Left;
            Top = top ?? Top;
            Width = width.Value;
            Height = height.Value;
        }

        // Auto-scroll message list when new messages arrive
        if (ViewModel.ActiveChannel != null)
            SubscribeAutoScroll(ViewModel.ActiveChannel);

        ViewModel.PropertyChanged += (s, args) =>
        {
            if (args.PropertyName == nameof(ShellViewModel.ActiveChannel) && ViewModel.ActiveChannel != null)
            {
                SubscribeAutoScroll(ViewModel.ActiveChannel);
                ComposeBox.Focus();
            }
        };

        // Try auto-login with saved broker token
        await ViewModel.TryAutoLoginAsync();
    }

    private void SubscribeAutoScroll(ChannelViewModel channel)
    {
        channel.Messages.CollectionChanged += (s, args) =>
        {
            if (args.Action == NotifyCollectionChangedAction.Add && channel.ScrollToBottom)
            {
                if (MessageList.Items.Count > 0)
                {
                    MessageList.ScrollIntoView(MessageList.Items[^1]);
                }
                channel.ScrollToBottom = false;
            }
        };
    }

    private void ComposeBox_KeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Enter && !Keyboard.Modifiers.HasFlag(ModifierKeys.Shift))
        {
            e.Handled = true;
            ViewModel.SendMessageCommand.Execute(null);
        }
        else if (e.Key == Key.Up && string.IsNullOrEmpty(ViewModel.ComposeText))
        {
            e.Handled = true;
            ViewModel.EditLastOwnMessageCommand.Execute(null);
        }
        else if (e.Key == Key.Escape)
        {
            e.Handled = true;
            ViewModel.CancelComposeModeCommand.Execute(null);
        }
    }

    private void ComposeBox_TextChanged(object sender, TextChangedEventArgs e)
    {
        ViewModel.OnComposeTextChanged();
    }

    private void JoinBox_KeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Enter && sender is TextBox tb)
        {
            e.Handled = true;
            var text = tb.Text.Trim();
            if (!string.IsNullOrEmpty(text))
            {
                ViewModel.JoinChannelCommand.Execute(text);
                tb.Text = "";
            }
        }
    }

    private void LoginModeTab_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button btn && btn.Tag is string mode)
        {
            ViewModel.LoginMode = mode;
        }
    }

    // ── Context menu (built programmatically to avoid XAML Style limitations) ──

    private void MessageItem_MouseRightButtonUp(object sender, MouseButtonEventArgs e)
    {
        if (sender is not FrameworkElement fe || fe.DataContext is not IrcMessage msg) return;

        var menu = new ContextMenu
        {
            Background = (System.Windows.Media.Brush)FindResource("BgSurface"),
            Foreground = (System.Windows.Media.Brush)FindResource("FgText"),
        };

        var reply = new MenuItem { Header = "Reply" };
        reply.Click += (_, _) => ViewModel.StartReplyCommand.Execute(msg);
        menu.Items.Add(reply);

        var copy = new MenuItem { Header = "Copy Text" };
        copy.Click += (_, _) => Clipboard.SetText(msg.Text);
        menu.Items.Add(copy);

        if (msg.IsSelf && !msg.IsSystem)
        {
            var edit = new MenuItem { Header = "Edit" };
            edit.Click += (_, _) => ViewModel.StartEditCommand.Execute(msg);
            menu.Items.Add(edit);

            var del = new MenuItem { Header = "Delete" };
            del.Click += (_, _) => ViewModel.DeleteMessageCommand.Execute(msg);
            menu.Items.Add(del);
        }

        menu.Items.Add(new Separator());

        var pin = new MenuItem { Header = "Pin" };
        pin.Click += (_, _) => ViewModel.PinMessageCommand.Execute(msg);
        menu.Items.Add(pin);

        var unpin = new MenuItem { Header = "Unpin" };
        unpin.Click += (_, _) => ViewModel.UnpinMessageCommand.Execute(msg);
        menu.Items.Add(unpin);

        menu.Items.Add(new Separator());

        var copyId = new MenuItem { Header = "Copy Message ID" };
        copyId.Click += (_, _) => Clipboard.SetText(msg.Id);
        menu.Items.Add(copyId);

        menu.IsOpen = true;
        e.Handled = true;
    }

    private void OnClosing(object? sender, System.ComponentModel.CancelEventArgs e)
    {
        ViewModel.SaveWindowBounds(Left, Top, Width, Height);
        ViewModel.Dispose();
    }
}
