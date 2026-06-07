//! Public Bluesky / AT-Proto lookups that let a being know *who* it's talking
//! to and *what they care about* — the engine behind the feed-aware cold open
//! and the proactive personalized greeting.
//!
//! Everything here uses the unauthenticated public AppView (`public.api.bsky.app`)
//! — read-only, no keys. A being never posts; it only reads, to be personal.

use std::time::Duration;

const APPVIEW: &str = "https://public.api.bsky.app/xrpc";
const TIMEOUT: Duration = Duration::from_secs(4);

/// Resolve a DID (`did:plc:…` / `did:web:…`) to its current Bluesky handle.
/// `None` for `did:key:` (guests, no profile) or any lookup failure. This is
/// what makes personalization robust: a being keys off the joiner's *identity*,
/// not whatever freeq nick they happen to be using.
pub async fn handle_for_did(http: &reqwest::Client, did: &str) -> Option<String> {
    if !did.starts_with("did:") || did.starts_with("did:key:") {
        return None;
    }
    profile_field(http, did, "handle").await
}

/// Resolve a handle to its DID (used for round-trip tests and handle→did needs).
pub async fn did_for_handle(http: &reqwest::Client, handle: &str) -> Option<String> {
    profile_field(http, handle, "did").await
}

async fn profile_field(http: &reqwest::Client, actor: &str, field: &str) -> Option<String> {
    let url = format!("{APPVIEW}/app.bsky.actor.getProfile?actor={actor}");
    let resp = tokio::time::timeout(TIMEOUT, http.get(&url).send()).await.ok()?.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    let v = json.get(field)?.as_str()?.to_string();
    (!v.is_empty()).then_some(v)
}

/// An actor's recent public posts (original posts, no replies), newest-trimmed.
/// `actor` may be a handle or a DID. Empty vec on any miss.
pub async fn recent_posts(http: &reqwest::Client, actor: &str, limit: u8) -> Vec<String> {
    let url = format!(
        "{APPVIEW}/app.bsky.feed.getAuthorFeed?actor={actor}&limit={limit}&filter=posts_no_replies"
    );
    let Ok(Ok(resp)) = tokio::time::timeout(TIMEOUT, http.get(&url).send()).await else {
        return Vec::new();
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let Ok(json) = resp.json::<serde_json::Value>().await else {
        return Vec::new();
    };
    json.get("feed")
        .and_then(|f| f.as_array())
        .map(|feed| {
            feed.iter()
                .filter_map(|it| it.pointer("/post/record/text").and_then(|t| t.as_str()))
                .map(|t| t.trim().replace('\n', " "))
                .filter(|t| !t.is_empty())
                .map(|t| t.chars().take(220).collect::<String>())
                .collect()
        })
        .unwrap_or_default()
}

/// Format recent posts as an LLM context block. `None` when there are none.
pub fn context_block(label: &str, posts: &[String]) -> Option<String> {
    if posts.is_empty() {
        return None;
    }
    let lines: String = posts.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n");
    Some(format!(
        "Recent public Bluesky posts by {label} — use to be personal and specific (react, don't recite):\n{lines}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http() -> reqwest::Client {
        reqwest::Client::new()
    }

    // End-to-end against the live public AppView. `chadfowler.com` is a stable
    // handle with public posts; if Bluesky is unreachable these will fail loudly
    // (that's the point of an e2e test).
    #[tokio::test]
    async fn recent_posts_returns_real_content() {
        let posts = recent_posts(&http(), "chadfowler.com", 4).await;
        assert!(!posts.is_empty(), "expected public posts for chadfowler.com");
        assert!(posts.iter().all(|p| !p.is_empty()));
    }

    #[tokio::test]
    async fn did_handle_round_trip() {
        let h = http();
        let did = did_for_handle(&h, "chadfowler.com").await.expect("handle → did");
        assert!(did.starts_with("did:"), "got {did}");
        let handle = handle_for_did(&h, &did).await.expect("did → handle");
        assert_eq!(handle, "chadfowler.com");
    }

    #[tokio::test]
    async fn did_key_has_no_handle() {
        // Guests (did:key) have no Bluesky profile → no personalization.
        let got = handle_for_did(&http(), "did:key:z6MkabcDEF").await;
        assert!(got.is_none());
    }

    #[test]
    fn context_block_formats_or_skips() {
        assert!(context_block("x", &[]).is_none());
        let b = context_block("alice", &["shipped a thing".into()]).unwrap();
        assert!(b.contains("alice") && b.contains("shipped a thing"));
    }
}
