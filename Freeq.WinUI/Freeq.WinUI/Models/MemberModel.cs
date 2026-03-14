namespace Freeq.WinUI.Models;

public enum MemberRole
{
    Operator,
    HalfOp,
    Voiced,
    Regular
}

public enum PresenceStatus
{
    Online,
    Away,
    Offline
}

public class MemberModel
{
    public string Nick { get; set; } = string.Empty;
    public string? Did { get; set; }
    public MemberRole Role { get; set; } = MemberRole.Regular;
    public PresenceStatus Status { get; set; } = PresenceStatus.Online;
    public bool IsVerified => !string.IsNullOrEmpty(Did);
    public string NickInitial => string.IsNullOrEmpty(Nick) ? "?" : Nick[..1].ToUpper();

    public string RoleBadge => Role switch
    {
        MemberRole.Operator => "@",
        MemberRole.HalfOp => "%",
        MemberRole.Voiced => "+",
        _ => ""
    };
}
