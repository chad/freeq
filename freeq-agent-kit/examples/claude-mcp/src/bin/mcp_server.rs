//! MCP server that lets a Claude Code agent join a freeq AV call.
//!
//! Tools exposed:
//!   - `freeq_connect` — join an existing AV session (or start one)
//!   - `freeq_listen`  — long-poll for transcribed utterances
//!   - `freeq_say`     — speak a line through TTS into the call
//!   - `freeq_disconnect` — leave the call
//!
//! Configuration is read from env at connect time:
//!   GROQ_API_KEY, ELEVENLABS_API_KEY,
//!   FREEQ_SERVER (default wss://irc.freeq.at/irc),
//!   FREEQ_ELEVEN_VOICE_ID, FREEQ_ELEVEN_MODEL, FREEQ_SFU_URL.

use std::sync::Arc;
use std::time::Duration;

use freeq_claude_mcp::{OrcConfig, Orchestrator, SayPriority, TileOverlay};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ConnectArgs {
    /// Channel to join, e.g. "#avtest". Must include the leading "#".
    pub channel: String,
    /// Nick to register under. Defaults to "claude".
    #[serde(default)]
    pub nick: Option<String>,
    /// did:key identity name (subdir under ~/.freeq/bots/). Defaults to `nick`.
    #[serde(default)]
    pub identity_name: Option<String>,
    /// If true and no AV session is active, send `av-start` to begin one.
    /// Default: false — sit on the channel and join whatever session a
    /// human starts later.
    #[serde(default)]
    pub start_if_idle: bool,
    /// Other agent nicks in the room (suppresses cross-agent address triggers).
    #[serde(default)]
    pub peer_agents: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ListenArgs {
    /// Max seconds to wait for the first transcript. Once one arrives,
    /// any others already buffered are returned with it.
    #[serde(default = "default_timeout_secs")]
    pub timeout_seconds: u32,
}

fn default_timeout_secs() -> u32 {
    30
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SayArgs {
    /// What to say aloud. Keep it short — humans converse in 1–2
    /// sentence turns and bots that monologue feel unnatural.
    pub text: String,
    /// "addressed" — you're answering a directly-addressed question.
    /// Always speaks.
    /// "volunteer" — you're surfacing something on your own (a
    /// correction, a missing fact, a high-value observation). Subject
    /// to cooldown to prevent room domination; if rejected the
    /// response carries `suppressed: true`.
    /// Default: "addressed".
    #[serde(default = "default_priority")]
    pub priority: SayPriority,
}

fn default_priority() -> SayPriority {
    SayPriority::Addressed
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct PostArgs {
    /// Text to drop into the IRC channel as a chat message. Use for
    /// links, citations, code snippets, diffs, decision lists, anything
    /// the human would want to scroll back to or copy. Multi-line text
    /// is split and posted line-by-line.
    pub text: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
// schemars emits a bare `oneOf` for an internally-tagged enum; Claude Code's
// MCP client rejects any tool whose inputSchema lacks `"type": "object"`,
// which drops the *entire* tools/list. Force the object type alongside the
// variant `oneOf`.
#[schemars(extend("type" = "object"))]
pub enum ShowArgs {
    /// A scene card — title at the top, up to 6 bullets below.
    Card {
        title: String,
        #[serde(default)]
        bullets: Vec<String>,
    },
    /// A quote — pulled-out italic text with optional attribution.
    Quote {
        text: String,
        #[serde(default)]
        source: Option<String>,
    },
    /// Clear the overlay and return to plain face.
    Clear,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ShowFileArgs {
    /// Path to the file on disk (relative to the MCP server's CWD,
    /// which is normally CC's working directory).
    pub path: String,
    /// First line to render (1-indexed). Default 1.
    #[serde(default = "default_one")]
    pub line_start: u32,
    /// Last line to render (1-indexed, inclusive). Default = line_start + 24.
    #[serde(default)]
    pub line_end: Option<u32>,
}

fn default_one() -> u32 {
    1
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct StatusArgs {
    /// Status label to display in the corner chip. Common values:
    /// "listening", "thinking", "presenting", "reading", "idle".
    /// Pass an empty string to clear.
    pub label: String,
    /// Also flip the working/thinking indicator (rotating arc on the
    /// face). Defaults to true when label is "thinking", else false.
    #[serde(default)]
    pub thinking: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct LookArgs {
    /// Which participant's video to inspect. If omitted, the bot picks
    /// the most-recently-active non-self participant. Accepts the short
    /// nick (e.g. "narrator") even when the actual broadcast nick has a
    /// DID suffix (e.g. "narrator-z6mk…").
    #[serde(default)]
    pub speaker: Option<String>,
    /// What to ask the vision model about the frame. Default is generic
    /// "what do you see"; pass something specific for better answers
    /// ("read the text on the slide", "what is the chart showing").
    #[serde(default)]
    pub question: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RecallArgs {
    /// Free-text query. Internally sanitised before FTS5 MATCH.
    pub query: String,
    /// Max number of past exchanges to return. Default 5.
    #[serde(default = "default_recall_limit")]
    pub limit: u32,
}

fn default_recall_limit() -> u32 {
    5
}

#[derive(Clone)]
pub struct FreeqClaudeHandler {
    orc: Arc<Mutex<Option<Arc<Orchestrator>>>>,
    tool_router: ToolRouter<FreeqClaudeHandler>,
}

#[tool_router]
impl FreeqClaudeHandler {
    pub fn new() -> Self {
        Self {
            orc: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "\
Join a freeq AV channel as a voice participant. Returns connection info \
once the AV session is up. If already connected, errors. Set start_if_idle=true \
to begin a session when none is active; otherwise the bot waits silently for \
a human to start one.")]
    async fn freeq_connect(
        &self,
        Parameters(args): Parameters<ConnectArgs>,
    ) -> Result<CallToolResult, McpError> {
        if self.orc.lock().await.is_some() {
            return Ok(error_text("already connected — call freeq_disconnect first"));
        }
        let nick = args.nick.unwrap_or_else(|| "claude".to_string());
        let identity_name = args.identity_name.unwrap_or_else(|| nick.clone());
        let cfg = OrcConfig {
            server: std::env::var("FREEQ_SERVER")
                .unwrap_or_else(|_| "wss://irc.freeq.at/irc".to_string()),
            channel: args.channel.clone(),
            nick: nick.clone(),
            identity_name,
            start_if_idle: args.start_if_idle,
            groq_api_key: env_nonempty("GROQ_API_KEY"),
            elevenlabs_api_key: env_nonempty("ELEVENLABS_API_KEY"),
            elevenlabs_voice_id: std::env::var("FREEQ_ELEVEN_VOICE_ID")
                .unwrap_or_else(|_| "aj0fZfXTBc7E3By4X8L2".to_string()),
            elevenlabs_model: std::env::var("FREEQ_ELEVEN_MODEL")
                .unwrap_or_else(|_| "eleven_turbo_v2_5".to_string()),
            sfu_url_override: std::env::var("FREEQ_SFU_URL").ok(),
            peer_agents: args.peer_agents,
            ghostly_character: std::env::var("FREEQ_CHARACTER")
                .unwrap_or_else(|_| "eliza".to_string()),
            volunteer_cooldown_secs: std::env::var("FREEQ_VOLUNTEER_COOLDOWN")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            emit_diagrams: std::env::var("FREEQ_EMIT_DIAGRAMS")
                .map(|s| s != "0" && s.to_lowercase() != "false")
                .unwrap_or(true),
            enable_memory: std::env::var("FREEQ_MEMORY")
                .map(|s| s != "0" && s.to_lowercase() != "false")
                .unwrap_or(true),
            barge_in: std::env::var("FREEQ_BARGE_IN")
                .map(|s| s != "0" && s.to_lowercase() != "false")
                .unwrap_or(true),
            deepgram_api_key: env_nonempty("DEEPGRAM_API_KEY"),
            deepgram_model: std::env::var("DEEPGRAM_MODEL")
                .unwrap_or_else(|_| "nova-3".to_string()),
        };
        match Orchestrator::connect(cfg).await {
            Ok(o) => {
                *self.orc.lock().await = Some(Arc::new(o));
                Ok(success_json(serde_json::json!({
                    "channel": args.channel,
                    "nick": nick,
                })))
            }
            Err(e) => Ok(error_text(&format!("connect failed: {e:#}"))),
        }
    }

    #[tool(description = "\
Wait up to `timeout_seconds` for the next batch of transcribed utterances. \
Returns an array of { speaker, text, addressed, question, timestamp_ms }. \
`addressed=true` means the line directly addressed the bot by name — in \
direct-address mode, reply only to those. `addressed=false` lines are \
context: things you can hear but should not respond to unless they're \
high-value to volunteer on. The call returns an empty array on timeout.")]
    async fn freeq_listen(
        &self,
        Parameters(args): Parameters<ListenArgs>,
    ) -> Result<CallToolResult, McpError> {
        let orc = {
            let g = self.orc.lock().await;
            g.as_ref().cloned()
        };
        let Some(orc) = orc else {
            return Ok(error_text("not connected — call freeq_connect first"));
        };
        let timeout = Duration::from_secs(args.timeout_seconds.clamp(1, 300) as u64);
        let batch = orc.recv_batch(timeout).await;
        Ok(success_json(serde_json::json!({
            "transcripts": batch.iter().map(|t| serde_json::json!({
                "speaker": t.speaker,
                "text": t.text,
                "addressed": t.addressed,
                "question": t.question,
                "timestamp_ms": t.timestamp_ms,
            })).collect::<Vec<_>>(),
        })))
    }

    #[tool(description = "\
Speak `text` into the call. The text is synthesized via ElevenLabs and \
broadcast to every participant; it also appears as a text PRIVMSG in the \
IRC channel for non-AV observers. Returns when the audio is queued — the \
speaker keeps playing after the call returns. Keep utterances short \
(1–2 sentences). Don't narrate tool calls or internal thinking.")]
    async fn freeq_say(
        &self,
        Parameters(args): Parameters<SayArgs>,
    ) -> Result<CallToolResult, McpError> {
        let orc = {
            let g = self.orc.lock().await;
            g.as_ref().cloned()
        };
        let Some(orc) = orc else {
            return Ok(error_text("not connected — call freeq_connect first"));
        };
        match orc.say(&args.text, args.priority).await {
            Ok(r) => Ok(success_json(serde_json::json!({
                "spoke": !r.suppressed,
                "suppressed": r.suppressed,
                "cooldown_remaining_secs": r.cooldown_remaining_secs,
                "text": args.text,
            }))),
            Err(e) => Ok(error_text(&format!("say failed: {e:#}"))),
        }
    }

    #[tool(description = "\
Drop `text` into the IRC channel as a chat message — no TTS, no voice. \
Use this for links, source citations, code snippets, diffs, bulleted \
decision lists, anything a human would want to scroll back to or copy. \
Multi-line text is split on newlines and posted line by line. Don't \
duplicate things you've already spoken; voice and chat are different \
bandwidths.")]
    async fn freeq_post(
        &self,
        Parameters(args): Parameters<PostArgs>,
    ) -> Result<CallToolResult, McpError> {
        let orc = {
            let g = self.orc.lock().await;
            g.as_ref().cloned()
        };
        let Some(orc) = orc else {
            return Ok(error_text("not connected — call freeq_connect first"));
        };
        match orc.post(&args.text).await {
            Ok(()) => Ok(success_json(serde_json::json!({ "posted": true }))),
            Err(e) => Ok(error_text(&format!("post failed: {e:#}"))),
        }
    }

    #[tool(description = "\
Project a scene card or quote onto your video tile, on top of the face. \
The card stays visible until replaced by another freeq_show or by \
freeq_show_file, or until cleared with `kind: \"clear\"`. Use this to \
surface key points without speaking — humans glance, don't read in full.")]
    async fn freeq_show(
        &self,
        Parameters(args): Parameters<ShowArgs>,
    ) -> Result<CallToolResult, McpError> {
        let orc = {
            let g = self.orc.lock().await;
            g.as_ref().cloned()
        };
        let Some(orc) = orc else {
            return Ok(error_text("not connected — call freeq_connect first"));
        };
        let overlay = match args {
            ShowArgs::Card { title, bullets } => TileOverlay::Card { title, bullets },
            ShowArgs::Quote { text, source } => TileOverlay::Quote { text, source },
            ShowArgs::Clear => TileOverlay::None,
        };
        orc.control.set_overlay(overlay);
        Ok(success_json(serde_json::json!({ "shown": true })))
    }

    #[tool(description = "\
Render a slice of a file as your video tile (replaces the face). Reads \
`path` from disk and shows lines [line_start, line_end] in monospace. \
Use this when discussing code or a config file with the room — humans see \
the exact lines you're talking about. Clear by calling freeq_show with \
`kind: \"clear\"`.")]
    async fn freeq_show_file(
        &self,
        Parameters(args): Parameters<ShowFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        let orc = {
            let g = self.orc.lock().await;
            g.as_ref().cloned()
        };
        let Some(orc) = orc else {
            return Ok(error_text("not connected — call freeq_connect first"));
        };
        let body = match std::fs::read_to_string(&args.path) {
            Ok(s) => s,
            Err(e) => return Ok(error_text(&format!("read {}: {e}", args.path))),
        };
        let start = args.line_start.max(1);
        let end = args.line_end.unwrap_or(start + 24).max(start);
        let lines: Vec<String> = body
            .lines()
            .skip((start - 1) as usize)
            .take((end - start + 1) as usize)
            .map(|s| s.to_string())
            .collect();
        let n = lines.len();
        orc.control.set_overlay(TileOverlay::File {
            path: args.path.clone(),
            lines,
            line_start: start,
        });
        Ok(success_json(serde_json::json!({
            "shown": true,
            "lines": n,
        })))
    }

    #[tool(description = "\
Flip the bot's visible status. Renders a small chip in the corner of the \
tile and (when relevant) flips the face's working/thinking arc. Use to \
ack that you heard something and are processing, without speaking. \
Common labels: \"listening\", \"thinking\", \"presenting\", \"reading\". \
Pass an empty label to clear.")]
    async fn freeq_set_status(
        &self,
        Parameters(args): Parameters<StatusArgs>,
    ) -> Result<CallToolResult, McpError> {
        let orc = {
            let g = self.orc.lock().await;
            g.as_ref().cloned()
        };
        let Some(orc) = orc else {
            return Ok(error_text("not connected — call freeq_connect first"));
        };
        let thinking = args
            .thinking
            .unwrap_or_else(|| args.label.eq_ignore_ascii_case("thinking"));
        orc.control.set_thinking(thinking);
        if args.label.is_empty() {
            orc.control.set_overlay(TileOverlay::None);
        } else {
            orc.control
                .set_overlay(TileOverlay::Status { label: args.label.clone() });
        }
        Ok(success_json(serde_json::json!({
            "status": args.label,
            "thinking": thinking,
        })))
    }

    #[tool(description = "\
Look at a participant's video — grab their most recent frame, send it to a \
vision model, return the description. Use this when someone shares their \
camera, screen, a slide, a diagram, or asks 'do you see this?' Pass \
`question` to ask something specific about the frame. Returns a 1–3 \
sentence description. Pair with freeq_say so the room hears what you saw.")]
    async fn freeq_look(
        &self,
        Parameters(args): Parameters<LookArgs>,
    ) -> Result<CallToolResult, McpError> {
        let orc = {
            let g = self.orc.lock().await;
            g.as_ref().cloned()
        };
        let Some(orc) = orc else {
            return Ok(error_text("not connected — call freeq_connect first"));
        };
        let Some(api_key) = env_nonempty("GROQ_API_KEY") else {
            return Ok(error_text("GROQ_API_KEY not set — vision unavailable"));
        };
        let model = std::env::var("FREEQ_VISION_MODEL")
            .unwrap_or_else(|_| "meta-llama/llama-4-scout-17b-16e-instruct".to_string());
        let Some((picked, frame)) = orc.latest_frame(args.speaker.as_deref()).await else {
            return Ok(error_text(&format!(
                "no video frame available{}",
                args.speaker
                    .as_deref()
                    .map(|s| format!(" for {s}"))
                    .unwrap_or_default()
            )));
        };
        let question = args
            .question
            .as_deref()
            .unwrap_or("What do you see in this frame? Reply in 1–3 short sentences.");
        let uri = match freeq_eliza::vision::frame_to_jpeg_data_uri(&frame) {
            Ok(u) => u,
            Err(e) => return Ok(error_text(&format!("jpeg encode failed: {e:#}"))),
        };
        let client = reqwest::Client::new();
        match freeq_eliza::vision::describe(&client, &api_key, &model, question, &uri).await {
            Ok(description) => Ok(success_json(serde_json::json!({
                "speaker": picked,
                "description": description,
            }))),
            Err(e) => Ok(error_text(&format!("vision describe failed: {e:#}"))),
        }
    }

    #[tool(description = "\
Search persistent memory for past exchanges in this channel. Returns at \
most `limit` matches as { speaker, text, ts } records. Use when a topic \
sounds familiar, when the user references prior conversation, or when you \
want context before answering. Returns empty when memory is off or there's \
no match.")]
    async fn freeq_recall(
        &self,
        Parameters(args): Parameters<RecallArgs>,
    ) -> Result<CallToolResult, McpError> {
        let orc = {
            let g = self.orc.lock().await;
            g.as_ref().cloned()
        };
        let Some(orc) = orc else {
            return Ok(error_text("not connected — call freeq_connect first"));
        };
        match orc.recall(&args.query, args.limit as usize) {
            Ok(recs) => Ok(success_json(serde_json::json!({
                "query": args.query,
                "hits": recs.iter().map(|r| serde_json::json!({
                    "speaker": r.asker,
                    "text": r.question,
                    "ts": r.ts,
                })).collect::<Vec<_>>(),
            }))),
            Err(e) => Ok(error_text(&format!("recall failed: {e:#}"))),
        }
    }

    #[tool(description = "List the current AV participants we know about (every nick we've subscribed to). Useful before freeq_look to pick which speaker.")]
    async fn freeq_participants(&self) -> Result<CallToolResult, McpError> {
        let orc = {
            let g = self.orc.lock().await;
            g.as_ref().cloned()
        };
        let Some(orc) = orc else {
            return Ok(error_text("not connected — call freeq_connect first"));
        };
        let nicks = orc.participants().await;
        Ok(success_json(serde_json::json!({ "nicks": nicks })))
    }

    #[tool(description = "Leave the AV call and quit IRC. Safe to call when not connected.")]
    async fn freeq_disconnect(&self) -> Result<CallToolResult, McpError> {
        let taken = self.orc.lock().await.take();
        let Some(orc) = taken else {
            return Ok(success_json(serde_json::json!({ "was_connected": false })));
        };
        let _ = orc.disconnect().await;
        Ok(success_json(serde_json::json!({ "was_connected": true })))
    }
}

#[tool_handler]
impl ServerHandler for FreeqClaudeHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Bridge a Claude Code agent into a freeq AV channel as a voice + \
                 chat + visual participant. Tools: freeq_connect, freeq_listen, \
                 freeq_say (priority=addressed|volunteer), freeq_post, freeq_show, \
                 freeq_show_file, freeq_set_status, freeq_disconnect. After connect, \
                 loop on freeq_listen. Route output by bandwidth: voice for the one \
                 sentence a human would want spoken; freeq_post for artifacts \
                 (links, code, decisions); freeq_show / freeq_show_file for \
                 persistent visual context on your tile. Direct-address replies are \
                 free; volunteer utterances are cooldowned. Don't narrate tool calls."
                    .to_string(),
            ),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Stdout is the MCP transport — log only to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(
                    "freeq_claude_mcp=info,freeq_eliza=info,freeq_av=info,info",
                )),
        )
        .init();

    let handler = FreeqClaudeHandler::new();
    let service = handler.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn success_json(value: serde_json::Value) -> CallToolResult {
    CallToolResult::success(vec![Content::text(value.to_string())])
}

fn error_text(msg: &str) -> CallToolResult {
    CallToolResult::error(vec![Content::text(msg.to_string())])
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.trim().is_empty())
}
