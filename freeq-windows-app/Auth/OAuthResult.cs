using System.Text.Json.Serialization;

namespace Freeq.Windows.Auth;

/// <summary>
/// OAuth result returned by the broker via redirect fragment.
/// </summary>
public class OAuthResult
{
    [JsonPropertyName("did")]
    public string? Did { get; set; }

    [JsonPropertyName("handle")]
    public string? Handle { get; set; }

    [JsonPropertyName("web_token")]
    public string? WebToken { get; set; }

    [JsonPropertyName("token")]
    public string? Token { get; set; }

    [JsonPropertyName("access_jwt")]
    public string? AccessJwt { get; set; }

    [JsonPropertyName("broker_token")]
    public string? BrokerToken { get; set; }

    [JsonPropertyName("pds_url")]
    public string? PdsUrl { get; set; }

    /// <summary>
    /// Get the effective web token (tries web_token, then token, then access_jwt).
    /// </summary>
    public string? EffectiveToken => WebToken ?? Token ?? AccessJwt;
}

/// <summary>
/// Response from the broker /session endpoint.
/// </summary>
public class BrokerSessionResponse
{
    [JsonPropertyName("token")]
    public string? Token { get; set; }

    [JsonPropertyName("did")]
    public string? Did { get; set; }

    [JsonPropertyName("handle")]
    public string? Handle { get; set; }

    [JsonPropertyName("nick")]
    public string? Nick { get; set; }
}
