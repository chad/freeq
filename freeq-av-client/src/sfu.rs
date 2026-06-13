//! SFU client — connect to freeq AV SFU via MoQ.
//!
//! Publishes mic audio as a MoQ broadcast and subscribes to other participants.
//! The iroh-live LocalBroadcast produces hang-formatted MoQ broadcasts (Opus audio)
//! which are compatible with the browser's moq-publish/moq-watch web components
//! (also Opus via libav-opus WASM). Both use the same MoQ catalog format through
//! the shared moq_relay::Cluster, enabling native <-> browser audio interop.

use anyhow::Result;
use iroh_live::media::{
    audio_backend::AudioBackend, codec::AudioCodec, format::AudioPreset, publish::LocalBroadcast,
    subscribe::RemoteBroadcast,
};

/// Connect to the SFU, publish mic audio, and subscribe to other participants.
pub async fn run_sfu(sfu_url: &str, session: &str, nick: &str) -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let broadcast_name = format!("{session}/{nick}");
    tracing::info!(%sfu_url, %broadcast_name, "Connecting to SFU via MoQ");

    // Create MoQ client
    let mut client_config = moq_native::ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    client_config.backend = Some(moq_native::QuicBackend::Noq);
    let client = client_config.init()?;

    // Set up audio capture + encoding via iroh-live
    let broadcast = LocalBroadcast::new();
    let audio_backend = AudioBackend::default();
    audio_backend.set_aec_enabled(false);

    let inputs = AudioBackend::list_inputs();
    let outputs = AudioBackend::list_outputs();
    println!("\n  Audio devices:");
    for d in &inputs {
        println!("    Input:  {}", d.name);
    }
    for d in &outputs {
        println!("    Output: {}", d.name);
    }

    let mic = audio_backend.default_input().await?;
    broadcast
        .audio()
        .set(mic, AudioCodec::Opus, [AudioPreset::Hq])?;
    println!("  Microphone active (Opus).");
    println!("  NOTE: Echo cancellation is disabled — use headphones to avoid feedback.");

    // LocalBroadcast already produces hang-formatted MoQ broadcasts.
    // Wire its BroadcastConsumer directly to the MoQ origin.
    let origin = moq_lite::Origin::produce();
    origin.publish_broadcast(&broadcast_name, broadcast.consume());
    tracing::info!(%broadcast_name, "Publishing audio broadcast via MoQ");

    // Subscription origin — receives other participants' broadcasts
    let sub_origin = moq_lite::Origin::produce();
    let mut sub_consumer = sub_origin.consume();

    // Connect to the SFU via WebSocket
    let base: url::Url = sfu_url.parse()?;
    let url = base.join("/av/moq")?;
    println!("  Connecting to {url}...");

    let session_handle = client
        .with_publish(origin.consume())
        .with_consume(sub_origin)
        .connect(url)
        .await?;

    println!("  Connected to SFU!");
    println!("  Publishing as: {broadcast_name}");
    println!("  Press Ctrl+C to leave.\n");

    // Show browser URL for easy joining
    println!("  -------------------------------------------------------");
    println!("  Browser call URL:");
    println!("  {sfu_url}/av/call.html?session={session}");
    println!("  -------------------------------------------------------\n");

    // Keep broadcast alive (dropping it stops encoding)
    let _broadcast = broadcast;

    // Watch for incoming broadcasts and play their audio
    let audio_for_playback = audio_backend.clone();
    let our_name = broadcast_name.clone();
    tokio::spawn(async move {
        while let Some((path, announce)) = sub_consumer.announced().await {
            match announce {
                Some(broadcast_consumer) => {
                    let path_str = path.to_string();

                    // Skip our own broadcast to avoid feedback loop
                    if path_str == our_name
                        || path_str
                            .ends_with(&format!("/{}", our_name.split('/').last().unwrap_or("")))
                    {
                        tracing::debug!("Skipping own broadcast: {path_str}");
                        continue;
                    }

                    println!("  + Broadcast announced: {path_str} (subscribing...)");

                    // Subscribe to audio only (skip video — native client is audio-only)
                    let ab = audio_for_playback.clone();
                    let ps = path_str.clone();
                    tokio::spawn(async move {
                        match RemoteBroadcast::new(&ps, broadcast_consumer).await {
                            Ok(remote) => {
                                match remote.audio(&ab).await {
                                    Ok(audio_track) => {
                                        println!("  ~ Receiving audio from {ps}");
                                        // Keep track alive until session ends
                                        let _track = audio_track;
                                        tokio::signal::ctrl_c().await.ok();
                                    }
                                    Err(e) => {
                                        tracing::warn!(%ps, "Failed to subscribe to audio: {e}")
                                    }
                                }
                            }
                            Err(e) => tracing::warn!(%ps, "Failed to read catalog: {e}"),
                        }
                    });
                }
                None => {
                    let path_str = path.to_string();
                    println!("  - Broadcast removed: {path_str}");
                }
            }
        }
        tracing::info!("Subscription stream ended");
    });

    // Wait for session to close or Ctrl+C
    tokio::select! {
        result = session_handle.closed() => {
            if let Err(e) = result {
                tracing::warn!("SFU session closed: {e}");
            }
            println!("  Session ended.");
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\n  Leaving...");
        }
    }

    Ok(())
}
