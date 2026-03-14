namespace Freeq.WinUI.Models;

public class MessageModel
{
    public string Id { get; set; } = string.Empty;
    public string Nick { get; set; } = string.Empty;
    public string Content { get; set; } = string.Empty;
    public DateTimeOffset Timestamp { get; set; }
    public bool IsEdited { get; set; }
    public bool IsDeleted { get; set; }
    public bool IsVerified { get; set; }
    public string? Did { get; set; }
    public bool IsSigned { get; set; }

    public string TimeString => Timestamp.LocalDateTime.ToString("h:mm tt");
    public string DateString => Timestamp.LocalDateTime.ToString("MMMM d, yyyy");
    public string NickInitial => string.IsNullOrEmpty(Nick) ? "?" : Nick[..1].ToUpper();
}
