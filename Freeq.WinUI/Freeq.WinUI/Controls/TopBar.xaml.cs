using System;
using Freeq.WinUI.Models;
using Freeq.WinUI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Freeq.WinUI.Controls;

public sealed partial class TopBar : UserControl
{
    public MainViewModel? ViewModel { get; set; }
    public event Action? MemberListToggled;

    public TopBar()
    {
        InitializeComponent();
    }

    public void Bind(MainViewModel vm)
    {
        ViewModel = vm;
        vm.PropertyChanged += (s, e) =>
        {
            switch (e.PropertyName)
            {
                case nameof(MainViewModel.SelectedChannel):
                    UpdateChannel(vm.SelectedChannel);
                    break;
                case nameof(MainViewModel.CurrentTopic):
                    TopicText.Text = vm.CurrentTopic;
                    break;
            }
        };

        vm.Operators.CollectionChanged += (s, e) => UpdateMemberCount();
        vm.Voiced.CollectionChanged += (s, e) => UpdateMemberCount();
        vm.Members.CollectionChanged += (s, e) => UpdateMemberCount();
    }

    private void UpdateChannel(ChannelModel? channel)
    {
        if (channel == null)
        {
            ChannelName.Text = "";
            TopicText.Text = "";
            ChannelIcon.Text = "";
            return;
        }

        ChannelIcon.Text = channel.Kind == ChannelKind.DirectMessage ? "\U0001F4AC" : "#";
        ChannelName.Text = channel.DisplayName;
        TopicText.Text = channel.Topic;
    }

    private void UpdateMemberCount()
    {
        if (ViewModel == null) return;
        MemberCountText.Text = ViewModel.TotalMemberCount.ToString();
    }

    private void OnMemberToggleClick(object sender, RoutedEventArgs e)
    {
        MemberListToggled?.Invoke();
    }
}
