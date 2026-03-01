using System.Text.Json;
using System.Text.Json.Serialization;

namespace Freeq.Windows.Interop;

/// <summary>
/// Versioned envelope wrapping every event from the Rust core.
/// Matches the Rust EventEnvelope struct serialization.
/// </summary>
public class EventEnvelope
{
    [JsonPropertyName("version")]
    public uint Version { get; set; }

    [JsonPropertyName("seq")]
    public ulong Seq { get; set; }

    [JsonPropertyName("timestamp_ms")]
    public long TimestampMs { get; set; }

    [JsonPropertyName("event")]
    public JsonElement Event { get; set; }

    /// <summary>
    /// Extract the "type" field from the event.
    /// </summary>
    public string? EventType => Event.TryGetProperty("type", out var t) ? t.GetString() : null;

    /// <summary>
    /// Extract the "data" field from the event as a JsonElement.
    /// </summary>
    public JsonElement? EventData => Event.TryGetProperty("data", out var d) ? d : null;

    /// <summary>
    /// Get a string property from the event data.
    /// </summary>
    public string? GetDataString(string key)
    {
        if (EventData is not JsonElement data) return null;
        return data.TryGetProperty(key, out var v) ? v.GetString() : null;
    }

    /// <summary>
    /// Get a bool property from the event data.
    /// </summary>
    public bool GetDataBool(string key)
    {
        if (EventData is not JsonElement data) return false;
        return data.TryGetProperty(key, out var v) && v.GetBoolean();
    }

    /// <summary>
    /// Get a nullable string property from the event data.
    /// </summary>
    public string? GetDataStringOrNull(string key)
    {
        if (EventData is not JsonElement data) return null;
        if (!data.TryGetProperty(key, out var v)) return null;
        return v.ValueKind == JsonValueKind.Null ? null : v.GetString();
    }
}
