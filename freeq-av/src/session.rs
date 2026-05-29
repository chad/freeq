//! [`AvSession`] — the MoQ transport layer for a freeq AV agent.
//!
//! A freeq voice/video call rides MoQ (Media over QUIC). Every
//! participant publishes one *broadcast* (their mic + camera) to a
//! shared SFU; everyone else subscribes to it. [`AvSession`] packages
//! both halves so an agent doesn't re-derive the plumbing:
//!
//! - **Publish** — connect to the SFU and publish the agent's own
//!   broadcast: an Opus audio track fed by a [`PushAudioSource`] (so the
//!   agent can speak) plus an H.264 video track fed by a caller-supplied
//!   [`VideoSource`].
//! - **Subscribe** — watch the SFU's announce stream and, for every
//!   *other* participant in the same session, subscribe to their audio,
//!   decode it, and surface a stream of [`PcmFrame`]s.
//!
//! The MoQ transport drops occasionally (network blips, SFU restarts).
//! [`AvSession`] reconnects automatically with backoff; a reconnect ends
//! every participant's PCM stream and re-announces them fresh.
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! use std::sync::Arc;
//! use std::sync::atomic::AtomicU32;
//! use freeq_av::{AvConfig, AvSession, Speaker, broadcast_path};
//! # use iroh_live::media::test_sources::TestPatternSource;
//! // A fresh video source per (re)connect — here a stand-in test pattern;
//! // a real agent renders its own tile.
//! let make_video = || TestPatternSource::new(640, 360);
//!
//! let (speaker, audio_source) = Speaker::new(Arc::new(AtomicU32::new(0)));
//! let config = AvConfig {
//!     sfu_url: "https://sfu.example/av/moq".parse()?,
//!     session_id: "abc123".into(),
//!     our_broadcast: broadcast_path("abc123", "eliza", "0a1b2c3d"),
//!     my_nick: "eliza".into(),
//! };
//! let mut session = AvSession::connect(config, audio_source, make_video);
//!
//! while let Some(mut participant) = session.recv().await {
//!     tokio::spawn(async move {
//!         while let Some(frame) = participant.audio.recv().await {
//!             // ... feed `frame` to a transcriber, recorder, meter ...
//!         }
//!     });
//! }
//! # Ok(()) }
//! ```

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};

use iroh_live::media::codec::{AudioCodec, VideoCodec};
use iroh_live::media::format::{AudioPreset, VideoFrame, VideoPreset};
use iroh_live::media::publish::LocalBroadcast;
use iroh_live::media::subscribe::RemoteBroadcast;
use iroh_live::media::traits::VideoSource;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

use crate::audio::{PcmFrame, PushAudioSource, TapBackend};

/// Where and how an agent joins an AV session.
pub struct AvConfig {
    /// The MoQ SFU URL — e.g. `https://host:8080/av/moq`.
    pub sfu_url: url::Url,
    /// The freeq session id. Every broadcast in the call is addressed
    /// `"{session_id}/..."`; the prefix lets [`AvSession`] ignore stale
    /// broadcasts the SFU still announces from prior sessions.
    pub session_id: String,
    /// The agent's own broadcast path — build it with [`broadcast_path`].
    /// Never tapped (subscribing to our own TTS would be a feedback
    /// loop).
    pub our_broadcast: String,
    /// The agent's nick. Any announced broadcast that resolves to this
    /// nick is skipped, regardless of its `~instance` suffix.
    pub my_nick: String,
}

/// A remote participant whose media [`AvSession`] is tapping.
///
/// `audio` yields decoded PCM until the participant leaves or the
/// session reconnects — either way the receiver simply closes. `video`
/// surfaces their most recent decoded frame, when they publish video.
pub struct AvParticipant {
    /// The participant's full broadcast path.
    pub path: String,
    /// The participant's display nick, parsed from the path.
    pub nick: String,
    /// Decoded PCM frames from the participant's audio track.
    pub audio: mpsc::Receiver<PcmFrame>,
    /// The participant's most recent video frame (see [`VideoHandle`]).
    pub video: VideoHandle,
}

/// A shared handle to a participant's most recent decoded video frame.
///
/// [`latest`](VideoHandle::latest) returns `None` until the first frame
/// decodes — and stays `None` for a participant who publishes no video
/// (audio-only). Cheap to clone.
#[derive(Clone, Default)]
pub struct VideoHandle {
    latest: Arc<Mutex<Option<VideoFrame>>>,
}

impl VideoHandle {
    /// The most recent frame, cloned out — frame pixel data is
    /// reference-counted, so the clone is shallow.
    pub fn latest(&self) -> Option<VideoFrame> {
        self.latest.lock().ok().and_then(|g| g.clone())
    }

    /// Replace the stored frame (called by the video pump).
    fn set(&self, frame: VideoFrame) {
        if let Ok(mut g) = self.latest.lock() {
            *g = Some(frame);
        }
    }
}

/// A live AV session: a background task that publishes the agent's
/// broadcast, taps every participant, and reconnects on transport loss.
///
/// Dropping the `AvSession` aborts that task — which ends the published
/// broadcast and every participant's PCM stream.
pub struct AvSession {
    participants: mpsc::Receiver<AvParticipant>,
    task: JoinHandle<()>,
}

impl AvSession {
    /// Open an AV session.
    ///
    /// Spawns a background task that connects to the SFU, publishes the
    /// agent's broadcast (`audio` for speech, `make_video()` for the
    /// video tile), and taps participants. Returns immediately — the
    /// connection is established (and re-established) inside the task.
    ///
    /// `make_video` is called once per (re)connect: the MoQ publisher
    /// consumes its [`VideoSource`], so each connection attempt needs a
    /// fresh one. For a shared renderer this is typically a cheap handle
    /// clone (`move || tile.source()`).
    pub fn connect<V, MkV>(
        config: AvConfig,
        audio: PushAudioSource,
        make_video: MkV,
    ) -> AvSession
    where
        V: VideoSource + Send + 'static,
        MkV: FnMut() -> V + Send + 'static,
    {
        let (tx, rx) = mpsc::channel(32);
        let task = tokio::spawn(run_subscriber(config, audio, make_video, tx));
        AvSession { participants: rx, task }
    }

    /// Receive the next participant the session has started tapping.
    ///
    /// Returns `None` only once the session has ended (the background
    /// task stopped). A reconnect re-announces every participant, so the
    /// same nick can surface here more than once over a session's life.
    pub async fn recv(&mut self) -> Option<AvParticipant> {
        self.participants.recv().await
    }
}

impl Drop for AvSession {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Build an agent's own broadcast path for a session.
///
/// freeq broadcasts are addressed `"{session_id}/{nick}~{instance}"` —
/// the `~{instance}` suffix lets one identity join from two devices
/// without a path collision.
pub fn broadcast_path(session_id: &str, nick: &str, instance: &str) -> String {
    format!("{session_id}/{nick}~{instance}")
}

/// Extract the display nick from a broadcast path.
///
/// A path is `"{session_id}/{nick}~{instance}"`; the nick is the final
/// `/`-segment up to its first `~`. A path with no `/` or no `~` still
/// yields a best-effort nick rather than panicking.
pub fn path_nick(path: &str) -> &str {
    let last = path.rsplit('/').next().unwrap_or(path);
    last.split('~').next().unwrap_or(last)
}

/// Decide whether an announced broadcast should be tapped.
///
/// The SFU announces *every* broadcast it knows: the agent's own, and
/// stale broadcasts from prior sessions on the same relay. Returns
/// `Some(nick)` only for a remote participant in *this* session.
fn should_tap<'a>(
    path: &'a str,
    session_id: &str,
    our_broadcast: &str,
    my_nick: &str,
) -> Option<&'a str> {
    if path == our_broadcast {
        return None;
    }
    // The trailing `/` is load-bearing: it stops session id `"sess"`
    // from matching a broadcast in session `"sess2"`.
    if !path.starts_with(&format!("{session_id}/")) {
        return None;
    }
    let nick = path_nick(path);
    if nick.eq_ignore_ascii_case(my_nick) {
        return None;
    }
    Some(nick)
}

/// Long-lived publisher/subscriber with automatic reconnect.
///
/// A MoQ session over the SFU does occasionally drop. Without reconnect
/// the agent would go permanently deaf + mute mid-call. This wraps
/// [`run_session`] in a backoff loop; the only thing that ends it is the
/// [`AvSession`] being dropped (which aborts this task).
async fn run_subscriber<V, MkV>(
    config: AvConfig,
    audio: PushAudioSource,
    mut make_video: MkV,
    tx: mpsc::Sender<AvParticipant>,
) where
    V: VideoSource + Send + 'static,
    MkV: FnMut() -> V + Send + 'static,
{
    let mut attempt: u32 = 0;
    loop {
        let started = Instant::now();
        // Fresh video source per attempt — the publisher consumes it.
        // The Speaker queue behind `audio` is shared, so a clone keeps
        // feeding the one queue.
        let result = run_session(&config, audio.clone(), make_video(), &tx).await;
        // A session that ran healthily then dropped resets the backoff —
        // only a tight failure loop escalates.
        if started.elapsed() > Duration::from_secs(30) {
            attempt = 0;
        }
        match &result {
            Ok(()) => tracing::info!("MoQ subscription stream ended cleanly"),
            Err(e) => tracing::warn!(error = ?e, "MoQ session error"),
        }
        // The AvSession was dropped — nobody is listening; stop.
        if tx.is_closed() {
            return;
        }
        attempt = attempt.saturating_add(1);
        let backoff = Duration::from_secs(2u64.pow(attempt.min(4))); // 2,4,8,16,16…
        tracing::info!(?backoff, attempt, "reconnecting MoQ session");
        tokio::time::sleep(backoff).await;
    }
}

/// One MoQ session: connect, publish the agent's broadcast, tap every
/// participant, until the transport drops. Tap tasks are owned by a
/// local [`JoinSet`] so when this returns (for any reason) they're all
/// aborted — a reconnect starts every tap fresh rather than leaving
/// zombies spinning on a dead transport.
async fn run_session<V>(
    config: &AvConfig,
    audio: PushAudioSource,
    video: V,
    tx: &mpsc::Sender<AvParticipant>,
) -> Result<()>
where
    V: VideoSource + Send + 'static,
{
    let mut client_config = moq_native::ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    client_config.backend = Some(moq_native::QuicBackend::Noq);
    let client = client_config.init()?;

    // Publish the agent's own broadcast — an Opus audio stream fed by
    // the PushAudioSource (silence until the agent speaks) plus an
    // H.264 video tile.
    let broadcast = LocalBroadcast::new();
    broadcast
        .audio()
        .set(audio, AudioCodec::Opus, [AudioPreset::Hq])
        .context("setting agent broadcast audio source")?;
    broadcast
        .video()
        .set_source(video, VideoCodec::H264, [VideoPreset::P360])
        .context("setting agent broadcast video source")?;
    let pub_origin = moq_lite::Origin::produce();
    pub_origin.publish_broadcast(config.our_broadcast.as_str(), broadcast.consume());

    let sub_origin = moq_lite::Origin::produce();
    let mut sub_consumer = sub_origin.consume();

    let session_handle = client
        .with_publish(pub_origin.consume())
        .with_consume(sub_origin)
        .connect(config.sfu_url.clone())
        .await
        .context("MoQ connect")?;

    // Keep the encoder alive for the session's lifetime.
    let _broadcast = broadcast;
    tracing::info!(
        our_broadcast = %config.our_broadcast,
        "MoQ connected — publishing agent broadcast, watching participants"
    );

    // Tap tasks live here — dropping the JoinSet on return aborts them.
    let mut taps: JoinSet<()> = JoinSet::new();
    let mut tapped: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            announced = sub_consumer.announced() => {
                match announced {
                    Some((path, Some(broadcast_consumer))) => {
                        let path = path.to_string();
                        let nick = match should_tap(
                            &path,
                            &config.session_id,
                            &config.our_broadcast,
                            &config.my_nick,
                        ) {
                            Some(n) => n.to_string(),
                            None => continue,
                        };
                        // The SFU can announce the same path twice — tap once.
                        if !tapped.insert(path.clone()) {
                            continue;
                        }
                        tracing::info!(%nick, %path, "new participant — subscribing");
                        taps.spawn(run_tap(tx.clone(), path, nick, broadcast_consumer));
                    }
                    Some((path, None)) => {
                        tracing::info!(path = %path.to_string(), "participant broadcast removed");
                    }
                    None => return Ok(()), // subscription stream closed
                }
            }
            res = session_handle.closed() => {
                anyhow::bail!("MoQ transport closed: {res:?}");
            }
        }
    }
}

/// One participant tap: subscribe to a remote broadcast's audio, decode
/// it, and forward decoded PCM as an [`AvParticipant`].
///
/// Holds the [`RemoteBroadcast`] + audio track alive until the
/// participant leaves or this task is aborted (on session reconnect).
/// When they drop, the decode pipeline tears down and the PCM receiver
/// the caller holds simply closes.
async fn run_tap(
    tx: mpsc::Sender<AvParticipant>,
    path: String,
    nick: String,
    broadcast_consumer: moq_lite::BroadcastConsumer,
) {
    let (backend, audio_rx) = TapBackend::channel();
    let remote = match RemoteBroadcast::new(&path, broadcast_consumer).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%nick, error = ?e, "RemoteBroadcast::new failed");
            return;
        }
    };
    // `audio_ready` blocks on the catalog watcher until the broadcast
    // advertises an audio rendition, then subscribes. The plain `audio()`
    // is a one-shot catalog read — a participant whose Opus track lands
    // a beat after the broadcast is announced would fail it permanently
    // with "no audio renditions".
    let track = match remote.audio_ready(&backend).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(%nick, error = ?e, "audio subscribe failed");
            return;
        }
    };
    tracing::info!(%nick, %path, "audio track live — tapping");

    // Dump what the SFU advertises for this participant — diagnostic for
    // "she doesn't see my screen" cases (no video rendition? unsupported
    // codec? camera off?).
    {
        let cat = remote.catalog();
        let video_renditions: Vec<String> =
            cat.video_renditions().map(str::to_string).collect();
        let audio_renditions: Vec<String> =
            cat.audio_renditions().map(str::to_string).collect();
        tracing::info!(
            %nick,
            has_video = remote.has_video(),
            has_audio = remote.has_audio(),
            video = ?video_renditions,
            audio = ?audio_renditions,
            "participant catalog",
        );
    }

    let video = VideoHandle::default();
    if tx
        .send(AvParticipant {
            path,
            nick: nick.clone(),
            audio: audio_rx,
            video: video.clone(),
        })
        .await
        .is_err()
    {
        return; // AvSession dropped — nobody is listening.
    }

    // Pump the participant's video into the shared latest-frame cell —
    // best-effort, since many participants are audio-only (`video_ready`
    // then just never resolves). Park on whichever ends first: the audio
    // track stopping, or the video pump finishing. `remote` + `track`
    // drop on return → pipelines tear down → the PCM receiver closes.
    let video_pump = async {
        match remote.video_ready().await {
            Ok(mut vtrack) => {
                tracing::info!(%nick, "video track live — watching");
                while let Some(frame) = vtrack.next_frame().await {
                    video.set(frame);
                }
            }
            Err(e) => tracing::warn!(%nick, error = ?e, "video subscribe failed"),
        }
    };
    tokio::select! {
        _ = track.stopped() => {}
        _ = video_pump => {}
    }
    tracing::info!(%nick, "participant tap ended");
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- path_nick ----------

    #[test]
    fn path_nick_strips_session_and_instance() {
        assert_eq!(path_nick("sess123/alice~0a1b2c3d"), "alice");
    }

    #[test]
    fn path_nick_without_instance_suffix() {
        assert_eq!(path_nick("sess123/bob"), "bob");
    }

    #[test]
    fn path_nick_without_session_prefix() {
        assert_eq!(path_nick("carol~ffeedd00"), "carol");
    }

    #[test]
    fn path_nick_handles_degenerate_paths() {
        assert_eq!(path_nick("plainnick"), "plainnick");
        assert_eq!(path_nick(""), "");
        // Trailing slash → empty final segment → empty nick (no panic).
        assert_eq!(path_nick("sess/"), "");
    }

    // ---------- broadcast_path ----------

    #[test]
    fn broadcast_path_round_trips_through_path_nick() {
        let path = broadcast_path("sess123", "eliza", "0a1b2c3d");
        assert_eq!(path, "sess123/eliza~0a1b2c3d");
        assert_eq!(path_nick(&path), "eliza");
    }

    // ---------- should_tap ----------

    #[test]
    fn should_tap_accepts_a_remote_participant() {
        assert_eq!(
            should_tap(
                "sess/alice~aabbccdd",
                "sess",
                "sess/eliza~11223344",
                "eliza",
            ),
            Some("alice"),
        );
    }

    #[test]
    fn should_tap_skips_our_exact_broadcast() {
        assert_eq!(
            should_tap(
                "sess/eliza~11223344",
                "sess",
                "sess/eliza~11223344",
                "eliza",
            ),
            None,
        );
    }

    #[test]
    fn should_tap_skips_our_nick_from_a_second_instance() {
        // Same identity joined from another device → different instance
        // suffix, so it isn't an exact match — but it's still us.
        assert_eq!(
            should_tap(
                "sess/eliza~99887766",
                "sess",
                "sess/eliza~11223344",
                "eliza",
            ),
            None,
        );
    }

    #[test]
    fn should_tap_self_filter_is_case_insensitive() {
        assert_eq!(
            should_tap("sess/Eliza~deadbeef", "sess", "sess/eliza~11223344", "eliza"),
            None,
        );
    }

    #[test]
    fn should_tap_skips_broadcasts_outside_the_session() {
        assert_eq!(
            should_tap(
                "otherssn/alice~aabbccdd",
                "sess",
                "sess/eliza~11223344",
                "eliza",
            ),
            None,
        );
    }

    #[test]
    fn should_tap_rejects_a_session_id_prefix_collision() {
        // Session "sess" must not match a broadcast in session "sess2";
        // the trailing `/` in the prefix check guards against it.
        assert_eq!(
            should_tap(
                "sess2/alice~aabbccdd",
                "sess",
                "sess/eliza~11223344",
                "eliza",
            ),
            None,
        );
    }

    #[test]
    fn should_tap_preserves_participant_nick_casing() {
        assert_eq!(
            should_tap("sess/BobLoblaw~01020304", "sess", "sess/eliza~1", "eliza"),
            Some("BobLoblaw"),
        );
    }

    // ---------- VideoHandle ----------

    #[test]
    fn video_handle_starts_empty_then_stores_and_shares_frames() {
        use iroh_live::media::format::VideoFrame;
        let h = VideoHandle::default();
        assert!(h.latest().is_none(), "no frame before the first set");

        let frame = VideoFrame::new_rgba(
            bytes::Bytes::from(vec![0u8; 4 * 2 * 2]),
            2,
            2,
            std::time::Duration::ZERO,
        );
        h.set(frame);
        assert_eq!(h.latest().expect("frame stored").dimensions, [2, 2]);

        // Clones share the one cell.
        let h2 = h.clone();
        assert!(h2.latest().is_some(), "clone sees the same frame");
    }
}
