//! Find an active AV session on a channel via the freeq REST API, and
//! derive the SFU URL from the IRC server URL.
//!
//! These mirror the helpers buried inside `freeq-eliza::irc`. Pulled
//! into this crate verbatim so we don't depend on `pub(crate)` symbols
//! over there.

use anyhow::{Context, Result, anyhow};

/// Derive the MoQ SFU URL from the IRC server URL.
/// `wss://host/irc` → `https://host/av/moq`.
/// Same shape as `freeq-eliza::irc::sfu_url_from_server`.
pub fn sfu_url_from_server(server: &str) -> Result<url::Url> {
    let trimmed = server.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("server URL is empty"));
    }
    let normalized = if trimmed.starts_with("ws://")
        || trimmed.starts_with("wss://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        trimmed.to_string()
    } else {
        format!("ws://{trimmed}")
    };
    let mut u: url::Url = normalized
        .parse()
        .with_context(|| format!("parsing server URL for SFU: {trimmed:?}"))?;
    match u.scheme() {
        "https" | "wss" => {
            u.set_scheme("https").ok();
        }
        "http" | "ws" => {
            u.set_scheme("http").ok();
        }
        other => return Err(anyhow!("unsupported scheme for SFU URL: {other:?}")),
    }
    if u.host_str().map(str::is_empty).unwrap_or(true) {
        return Err(anyhow!("server URL has no host: {trimmed:?}"));
    }
    u.set_path("/av/moq");
    Ok(u)
}

/// Derive the REST API base (`https://host[:port]`) from the IRC URL.
pub fn api_base_from_server(server: &str) -> Result<String> {
    let trimmed = server.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("server URL is empty"));
    }
    let normalized = if trimmed.starts_with("ws://")
        || trimmed.starts_with("wss://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        trimmed.to_string()
    } else {
        format!("ws://{trimmed}")
    };
    let u: url::Url = normalized
        .parse()
        .with_context(|| format!("parsing server URL for REST API: {trimmed:?}"))?;
    let scheme = match u.scheme() {
        "https" | "wss" => "https",
        "http" | "ws" => "http",
        other => return Err(anyhow!("unsupported scheme for REST API: {other:?}")),
    };
    let host = u.host_str().context("server URL has no host")?;
    Ok(match u.port() {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Look up the currently-active AV session id on `channel`, if any.
/// Returns None when there's no session, or on any HTTP/JSON error
/// (the caller treats "no session" and "lookup failed" the same way —
/// they either wait for a TAGMSG or send `av-start` themselves).
pub async fn discover_active_session(
    http: &reqwest::Client,
    server: &str,
    channel: &str,
) -> Option<String> {
    let base = api_base_from_server(server).ok()?;
    let encoded: String = channel
        .bytes()
        .map(|b| {
            if b == b'#' {
                "%23".to_string()
            } else {
                (b as char).to_string()
            }
        })
        .collect();
    let url = format!("{base}/api/v1/channels/{encoded}/sessions");
    let resp = http
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    let active = json.get("active")?;
    if active.is_null() {
        return None;
    }
    let state = active.get("state").and_then(|s| s.as_str()).unwrap_or("");
    if state != "Active" {
        return None;
    }
    active
        .get("id")
        .and_then(|i| i.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sfu_wss_to_https() {
        let u = sfu_url_from_server("wss://irc.freeq.at/irc").unwrap();
        assert_eq!(u.as_str(), "https://irc.freeq.at/av/moq");
    }

    #[test]
    fn api_base_strips_path() {
        assert_eq!(
            api_base_from_server("wss://irc.freeq.at/irc").unwrap(),
            "https://irc.freeq.at"
        );
    }

    #[test]
    fn api_base_keeps_port() {
        assert_eq!(
            api_base_from_server("ws://localhost:6667").unwrap(),
            "http://localhost:6667"
        );
    }

    #[test]
    fn sfu_empty_rejected() {
        assert!(sfu_url_from_server("").is_err());
    }
}
