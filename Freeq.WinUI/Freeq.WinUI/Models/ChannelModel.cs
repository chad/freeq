namespace Freeq.WinUI.Models;

public enum ChannelKind
{
    Server,
    Channel,
    DirectMessage
}

public class ChannelModel
{
    public string Name { get; set; } = string.Empty;
    public ChannelKind Kind { get; set; } = ChannelKind.Channel;
    public string Topic { get; set; } = string.Empty;
    public bool IsFavorite { get; set; }
    public bool IsMuted { get; set; }
    public int UnreadCount { get; set; }
    public int MentionCount { get; set; }
    public string? LastMessage { get; set; }
    public DateTimeOffset? LastMessageTime { get; set; }

    public string DisplayName => Kind == ChannelKind.DirectMessage
        ? Name.TrimStart('#')
        : Name;
}
