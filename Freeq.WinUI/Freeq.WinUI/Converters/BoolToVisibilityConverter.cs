using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Data;

namespace Freeq.WinUI.Converters;

public class BoolToVisibilityConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, string language)
    {
        bool invert = parameter is string s && s == "Invert";
        bool visible = value is bool b && b;
        if (invert) visible = !visible;
        return visible ? Visibility.Visible : Visibility.Collapsed;
    }

    public object ConvertBack(object value, Type targetType, object parameter, string language)
    {
        return value is Visibility v && v == Visibility.Visible;
    }
}

public class CountToVisibilityConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, string language)
    {
        return value is int count && count > 0 ? Visibility.Visible : Visibility.Collapsed;
    }

    public object ConvertBack(object value, Type targetType, object parameter, string language)
    {
        throw new NotImplementedException();
    }
}

public class NickToColorConverter : IValueConverter
{
    private static readonly string[] NickColors =
    [
        "#FF6EB4", "#00D4AA", "#FFB547", "#5C9EFF", "#B18CFF",
        "#FF9547", "#00C4FF", "#FF5C5C", "#7EDD7E", "#FF85D0"
    ];

    public object Convert(object value, Type targetType, object parameter, string language)
    {
        if (value is not string nick || string.IsNullOrEmpty(nick))
            return NickColors[0];

        int hash = 0;
        foreach (char c in nick)
            hash = (hash * 31 + c) & 0x7FFFFFFF;

        return NickColors[hash % NickColors.Length];
    }

    public object ConvertBack(object value, Type targetType, object parameter, string language)
    {
        throw new NotImplementedException();
    }
}
