namespace Freeq.Windows.Models;

public class IrcMessage
{
    public string Id { get; set; } = "";
    public string From { get; set; } = "";
    public string Target { get; set; } = "";
    public string Text { get; set; } = "";
    public DateTime Timestamp { get; set; } = DateTime.UtcNow;
    public bool IsAction { get; set; }
    public bool IsSelf { get; set; }
    public bool IsSystem { get; set; }
    public string? ReplyTo { get; set; }
    public string? ReplyPreview { get; set; }
    public string? EditOf { get; set; }
    public bool IsEdited { get; set; }
    public bool Deleted { get; set; }
    public bool IsBatchMessage { get; set; }

    /// <summary>
    /// Reactions: emoji → list of nicks who reacted.
    /// </summary>
    public Dictionary<string, List<string>> Reactions { get; set; } = new();

    public bool HasReactions => Reactions.Count > 0;

    public string ReactionsDisplay
    {
        get
        {
            if (Reactions.Count == 0) return "";
            return string.Join("  ", Reactions.Select(r => $"{r.Key} {r.Value.Count}"));
        }
    }

    public void AddReaction(string emoji, string nick)
    {
        if (!Reactions.ContainsKey(emoji))
            Reactions[emoji] = new List<string>();
        if (!Reactions[emoji].Contains(nick, StringComparer.OrdinalIgnoreCase))
            Reactions[emoji].Add(nick);
    }
}

public class IrcMember
{
    public string Nick { get; set; } = "";
    public bool IsOp { get; set; }
    public bool IsHalfOp { get; set; }
    public bool IsVoiced { get; set; }
    public string? AwayMsg { get; set; }

    public string PrefixedNick =>
        IsOp ? $"@{Nick}" :
        IsHalfOp ? $"%{Nick}" :
        IsVoiced ? $"+{Nick}" :
        Nick;
}

public class IrcChannel
{
    public string Name { get; set; } = "";
    public string Topic { get; set; } = "";
    public string? TopicSetBy { get; set; }
    public int UnreadCount { get; set; }
    public bool IsJoined { get; set; }
}
