//! Artifact generation pipeline for AV sessions.
//!
//! Defines traits for transcript and summary generation, with a stub
//! implementation for development. Real backends (Whisper, LLM) plug in
//! by implementing these traits.

use crate::av::{ArtifactKind, ArtifactVisibility, AvArtifact, AvSession};

/// Trait for generating transcripts from recorded audio.
pub trait TranscriptBackend: Send + Sync {
    /// Generate a transcript from audio data.
    fn transcribe(
        &self,
        audio_url: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>;
}

/// Trait for generating summaries from text (transcripts, chat logs).
pub trait SummaryBackend: Send + Sync {
    /// Generate a summary from a transcript.
    fn summarize(
        &self,
        transcript: &str,
        context: &SummaryContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SummaryResult, String>> + Send + '_>,
    >;
}

/// Context provided to the summary backend.
pub struct SummaryContext {
    pub session_title: Option<String>,
    pub channel: Option<String>,
    pub participants: Vec<String>, // nicks
    pub duration_secs: i64,
}

/// Structured summary output.
pub struct SummaryResult {
    pub summary: String,
    pub decisions: Vec<String>,
    pub action_items: Vec<ActionItem>,
}

pub struct ActionItem {
    pub description: String,
    pub assignee: Option<String>, // nick or DID
}

/// Stub transcript backend — returns placeholder text (for development).
pub struct StubTranscriptBackend;

impl TranscriptBackend for StubTranscriptBackend {
    fn transcribe(
        &self,
        audio_url: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        let url = audio_url.to_string();
        Box::pin(async move {
            Ok(format!(
                "[Transcript stub — recording at {url}]\n\nTranscript generation requires a Whisper backend.\nConfigure TRANSCRIPT_BACKEND=whisper to enable."
            ))
        })
    }
}

/// Stub summary backend — returns placeholder (for development).
pub struct StubSummaryBackend;

impl SummaryBackend for StubSummaryBackend {
    fn summarize(
        &self,
        transcript: &str,
        context: &SummaryContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SummaryResult, String>> + Send + '_>,
    > {
        let participant_list = context.participants.join(", ");
        let duration_min = context.duration_secs / 60;
        let channel = context.channel.as_deref().unwrap_or("ad-hoc").to_string();
        let title = context
            .session_title
            .as_deref()
            .unwrap_or("Voice session")
            .to_string();
        let transcript_preview = if transcript.len() > 200 {
            transcript[..200].to_string()
        } else {
            transcript.to_string()
        };

        Box::pin(async move {
            Ok(SummaryResult {
                summary: format!(
                    "## {title}\n\n**Channel:** {channel}\n**Participants:** {participant_list}\n**Duration:** {duration_min} min\n\n{transcript_preview}\n\n*Summary generation requires an LLM backend. Configure SUMMARY_BACKEND=llm to enable.*",
                ),
                decisions: vec![],
                action_items: vec![],
            })
        })
    }
}

/// Generate artifacts for a completed session.
/// Called when a session ends. Creates transcript + summary artifacts
/// and stores them in the database.
pub async fn generate_session_artifacts(
    session: &AvSession,
    state: &std::sync::Arc<crate::server::SharedState>,
    summary_backend: &dyn SummaryBackend,
) {
    let session_id = &session.id;
    let channel = session.channel.as_deref();

    // Calculate duration
    let ended_at = match &session.state {
        crate::av::AvSessionState::Ended { ended_at, .. } => *ended_at,
        _ => chrono::Utc::now().timestamp(),
    };
    let duration_secs = ended_at - session.created_at;

    // Skip very short sessions (< 30 seconds — probably accidental)
    if duration_secs < 30 {
        tracing::debug!(
            session_id,
            duration_secs,
            "Skipping artifact generation for short session"
        );
        return;
    }

    let participants: Vec<String> = session
        .participants
        .values()
        .map(|p| p.nick.clone())
        .collect();

    // For now, transcript requires a recording URL. Without iroh-live recording,
    // we generate a session summary from metadata only.
    let context = SummaryContext {
        session_title: session.title.clone(),
        channel: channel.map(|s| s.to_string()),
        participants: participants.clone(),
        duration_secs,
    };

    // Generate summary artifact
    let transcript_text = format!(
        "Session: {}\nChannel: {}\nParticipants: {}\nDuration: {} min\nStarted: {}\n",
        session.title.as_deref().unwrap_or("Voice session"),
        channel.unwrap_or("ad-hoc"),
        participants.join(", "),
        duration_secs / 60,
        session.created_at,
    );

    match summary_backend.summarize(&transcript_text, &context).await {
        Ok(result) => {
            let summary_artifact = AvArtifact {
                id: ulid::Ulid::new().to_string(),
                session_id: session_id.clone(),
                kind: ArtifactKind::Summary,
                created_at: chrono::Utc::now().timestamp(),
                created_by: None,
                content_ref: format!("inline:{}", result.summary),
                content_type: "text/markdown".to_string(),
                visibility: ArtifactVisibility::Channel,
                title: Some("Session Summary".to_string()),
            };
            state.with_db(|db| db.save_av_artifact(&summary_artifact));

            // Post summary to channel
            if let Some(ch) = channel {
                let short_summary = if result.summary.len() > 200 {
                    format!("{}...", &result.summary[..200])
                } else {
                    result.summary.clone()
                };
                crate::connection::messaging::broadcast_av_notice(
                    state,
                    ch,
                    &format!("Session summary: {short_summary}"),
                );
            }

            // Generate decision artifacts if any
            for (i, decision) in result.decisions.iter().enumerate() {
                let artifact = AvArtifact {
                    id: ulid::Ulid::new().to_string(),
                    session_id: session_id.clone(),
                    kind: ArtifactKind::Decisions,
                    created_at: chrono::Utc::now().timestamp(),
                    created_by: None,
                    content_ref: format!("inline:{decision}"),
                    content_type: "text/plain".to_string(),
                    visibility: ArtifactVisibility::Channel,
                    title: Some(format!("Decision #{}", i + 1)),
                };
                state.with_db(|db| db.save_av_artifact(&artifact));
            }

            // Generate task artifacts if any
            for item in &result.action_items {
                let artifact = AvArtifact {
                    id: ulid::Ulid::new().to_string(),
                    session_id: session_id.clone(),
                    kind: ArtifactKind::Tasks,
                    created_at: chrono::Utc::now().timestamp(),
                    created_by: None,
                    content_ref: format!("inline:{}", item.description),
                    content_type: "text/plain".to_string(),
                    visibility: ArtifactVisibility::Channel,
                    title: Some(format!("Action: {}", item.description)),
                };
                state.with_db(|db| db.save_av_artifact(&artifact));
            }

            tracing::info!(session_id, "Generated session artifacts");
        }
        Err(e) => {
            tracing::warn!(session_id, error = %e, "Failed to generate session summary");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_transcript_returns_placeholder() {
        let backend = StubTranscriptBackend;
        let result = backend.transcribe("https://example.com/audio.ogg").await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Transcript stub"));
    }

    #[tokio::test]
    async fn stub_summary_returns_structured_result() {
        let backend = StubSummaryBackend;
        let ctx = SummaryContext {
            session_title: Some("Standup".to_string()),
            channel: Some("#team".to_string()),
            participants: vec!["alice".to_string(), "bob".to_string()],
            duration_secs: 600,
        };
        let result = backend.summarize("test transcript", &ctx).await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.summary.contains("Standup"));
        assert!(r.summary.contains("#team"));
        assert!(r.summary.contains("alice"));
    }
}
