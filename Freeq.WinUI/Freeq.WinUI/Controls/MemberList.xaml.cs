using Freeq.WinUI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Freeq.WinUI.Controls;

public sealed partial class MemberList : UserControl
{
    public MemberList()
    {
        InitializeComponent();
    }

    public void Bind(MainViewModel vm)
    {
        OpsList.ItemsSource = vm.Operators;
        VoicedList.ItemsSource = vm.Voiced;
        MembersList.ItemsSource = vm.Members;

        vm.Operators.CollectionChanged += (s, e) =>
        {
            OpsSection.Visibility = vm.Operators.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
            OpsHeader.Text = $"OPERATORS — {vm.Operators.Count}";
        };

        vm.Voiced.CollectionChanged += (s, e) =>
        {
            VoicedSection.Visibility = vm.Voiced.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
            VoicedHeader.Text = $"VOICED — {vm.Voiced.Count}";
        };

        vm.Members.CollectionChanged += (s, e) =>
        {
            MembersHeader.Text = $"MEMBERS — {vm.Members.Count}";
        };
    }

    public static Visibility BoolToVis(bool value) =>
        value ? Visibility.Visible : Visibility.Collapsed;
}
