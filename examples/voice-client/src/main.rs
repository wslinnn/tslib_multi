//! Voice client example
//!
//! This example demonstrates how to create a voice-enabled client that:
//! - Connects to a TeamSpeak server
//! - Captures and transmits audio
//! - Receives and plays audio from other users

use anyhow::Result;
use clap::Parser;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

use tslib_audio::{AudioConfig, AudioManager};
use tslib_core::{Client, ClientConfig, Event, Identity};
use tslib_core::events::AudioCodec;

/// Voice-enabled TeamSpeak Client
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Server address (host:port)
    #[arg(short, long)]
    server: String,

    /// Client nickname
    #[arg(short, long, default_value = "TsLibVoice")]
    nickname: String,

    /// Server password (optional)
    #[arg(short, long)]
    password: Option<String>,

    /// Identity file path
    #[arg(short, long, default_value = "identity.json")]
    identity: String,

    /// Channel to join
    #[arg(short, long)]
    channel: Option<String>,

    /// Push-to-talk mode (default is voice activity)
    #[arg(long)]
    ptt: bool,

    /// Voice activity threshold (0.0-1.0)
    #[arg(long, default_value = "0.3")]
    vad_threshold: f32,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let args = Args::parse();

    // Load or create identity
    let identity = if std::path::Path::new(&args.identity).exists() {
        info!("Loading identity from {}", args.identity);
        Identity::load(&args.identity)?
    } else {
        info!("Creating new identity");
        let identity = Identity::create()?;
        identity.save(&args.identity)?;
        identity
    };

    info!("Identity: {}", identity.unique_id());

    // Configure audio
    let mut audio_config = AudioConfig::default();
    audio_config.vad_enabled = !args.ptt;
    audio_config.vad_threshold = args.vad_threshold;

    info!(
        "Audio mode: {}",
        if args.ptt {
            "Push-to-talk"
        } else {
            "Voice activity"
        }
    );

    // Create audio manager
    let audio_manager = AudioManager::new(audio_config)?;

    // Build client configuration
    let mut config_builder = ClientConfig::builder()
        .address(&args.server)
        .identity(identity)
        .nickname(&args.nickname);

    if let Some(password) = args.password {
        config_builder = config_builder.password(password);
    }

    if let Some(channel) = &args.channel {
        config_builder = config_builder.channel(channel);
    }

    let config = config_builder.build()?;

    // Connect to server (synchronous)
    info!("Connecting to {}...", args.server);
    let mut client = Client::connect(config)?;

    // Wait for connection to be established
    info!("Waiting for connection...");
    client.wait_connected().await?;

    info!("Connected!");

    // Subscribe to events
    let _events = client.subscribe();

    // Start audio capture
    info!("Starting audio capture...");
    let mut audio_rx = audio_manager.start_capture().await?;

    // Start audio playback
    audio_manager.start_playback().await?;

    info!("Voice client is running. Press Ctrl+C to exit.");
    info!("Commands:");
    info!("  - Speak to transmit audio (VAD mode)");
    info!("  - Ctrl+C to disconnect");

    // Main event loop
    let mut running = true;
    while running {
        // Process client events
        if let Ok(client_events) = client.process_events().await {
            for event in client_events {
                match event {
                    Event::TextMessage {
                        sender_name,
                        message,
                        ..
                    } => {
                        info!("[Chat] {}: {}", sender_name, message);
                    }
                    Event::UserJoined { user } => {
                        info!("[Join] {} joined the server", user.nickname);
                    }
                    Event::UserLeft { user, reason } => {
                        info!("[Leave] {} left: {}", user.nickname, reason);
                    }
                    Event::TalkStatusStart { user_id, .. } => {
                        info!("[Talk] User {} started talking", user_id);
                    }
                    Event::TalkStatusStop { user_id } => {
                        info!("[Talk] User {} stopped talking", user_id);
                    }
                    Event::AudioReceived { user_id, data, .. } => {
                        // Process incoming audio
                        if let Err(e) = audio_manager.process_incoming(user_id, &data, 4).await {
                            warn!("Audio processing error: {}", e);
                        }
                    }
                    Event::Disconnected { reason } => {
                        info!("Disconnected: {}", reason);
                        running = false;
                    }
                    _ => {}
                }
            }
        }

        // Handle captured audio (non-blocking check)
        while let Ok(packet) = audio_rx.try_recv() {
            if packet.voice_activity {
                if let Err(e) = client.send_audio(&packet.data, AudioCodec::OpusVoice) {
                    warn!("Failed to send audio: {}", e);
                }
            }
        }

        // Check for Ctrl+C
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down...");
                running = false;
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(10)) => {}
        }
    }

    // Cleanup
    audio_manager.stop_capture().await?;
    audio_manager.stop_playback().await?;
    client.disconnect()?;

    info!("Goodbye!");
    Ok(())
}
