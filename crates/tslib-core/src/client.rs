//! Main client implementation using tsclientlib
//!
//! The client wraps tsclientlib's Connection and provides a higher-level API.
//! Note: The client is not Send/Sync due to tsclientlib limitations.

use crate::config::{ClientConfig, FileTransferConfig};
use crate::connection::ConnectionState;
use crate::error::{ConnectionError, Error, Result};
use crate::events::{AudioCodec, Event, EventHandler};
use crate::state::{Channel, MaxClientsKind, PermissionHints, ServerInfo, ServerState, User};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use base64::Engine as _;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt,
};
use tsclientlib::prelude::*;
use tsclientlib::{Connection as TsConnection, DisconnectOptions, InMessage, PacketStat, Reason, StreamItem, ClientId, ChannelId};
use tsclientlib::MessageTarget as TsMessageTarget;
use tsclientlib::data::{Client as TsClient, Channel as TsChannel};
use tsclientlib::events::{Event as TsEvent, PropertyId};
use tsproto_types::{ChannelType as TsChannelType, ClientType as TsClientType, Codec as TsCodec};
use tsproto_types::errors::Error as TsError;
use tsproto_packets::packets::{AudioData, CodecType, Direction, Flags, OutAudio, OutCommand, PacketType};

/// A file entry from a channel's file list
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub datetime: i64,
    pub is_file: bool,
}

/// A snapshot of connection quality metrics from the underlying UDP layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NetworkStats {
    /// Smoothed round trip time in milliseconds.
    pub rtt_ms: f64,
    /// Round trip time deviation in milliseconds.
    pub rtt_dev_ms: f64,
    /// Average incoming+outgoing packet loss as a fraction (0.0–1.0),
    /// cumulative since the connection was established.
    pub packet_loss: f32,
    /// Incoming (server→client) packet loss as a fraction (0.0–1.0).
    pub packet_loss_in: f32,
    /// Bytes received during the last second.
    pub bytes_received_per_sec: u64,
    /// Bytes sent during the last second.
    pub bytes_sent_per_sec: u64,
    /// Cumulative packets observed by the loss accounting — same denominator
    /// as `packet_loss`, so clients can derive recent-window loss by
    /// differencing (`Δlost = loss·observed − prev_loss·prev_observed`).
    pub loss_observed_total: u64,
}

/// The main TeamSpeak client
///
/// This client wraps tsclientlib's Connection. Due to tsclientlib's design,
/// the client is not Send/Sync and must be used from a single task.
///
/// Use `process_events()` to poll for and handle server events.
/// Duration after which a user is considered to have stopped talking (ms)
const TALK_TIMEOUT_MS: u128 = 300;

/// After the first stream item, wait only this long for further items so a
/// burst returns in one batch while the first frame never waits extra.
const EVENT_BURST_GRACE_MS: u64 = 3;

/// Upper bound on stream items processed per wait_events call.
const MAX_STREAM_BURST: usize = 16;

/// Voice packet overhead: 2-byte voice id, 2-byte client id, 1-byte codec,
/// and the 8-byte MAC.
const AUDIO_PACKET_OVERHEAD: usize = 5 + 8;

/// Maximum encoded voice payload that fits in one UDP packet.
const MAX_AUDIO_PAYLOAD: usize = tsproto::MAX_UDP_PACKET_LENGTH - AUDIO_PACKET_OVERHEAD;

/// Protocol limits for direct whispers.
const MAX_WHISPER_CLIENTS: usize = 32;
const MAX_WHISPER_CHANNELS: usize = 255;

/// In-flight `ftgetfilelist` request, keyed by the server return code.
struct PendingFileList {
    channel_id: u64,
    path: String,
    entries: Vec<FileEntry>,
}

/// In-flight `permoverview` request, keyed by the server return code.
struct PendingPermOverview {
    channel_id: u64,
    perms: HashMap<u32, i32>,
}

/// In-flight upload; kept until the file transfer stream finishes.
struct PendingUpload {
    channel_id: u64,
    path: String,
    size: u64,
    source: UploadSource,
    password: Option<String>,
}

enum UploadSource {
    Memory(Vec<u8>),
    File(PathBuf),
}

/// In-flight download; `destination` writes to disk instead of memory.
struct PendingDownload {
    channel_id: u64,
    path: String,
    destination: Option<PathBuf>,
    offset: u64,
}

/// In-flight public `ftgetfileinfo` query.
struct PendingFileInfoQuery {
    channel_id: u64,
    path: String,
}

/// In-flight `ftgetfileinfo` verification, keyed by the server return code.
struct PendingFileInfo {
    channel_id: u64,
    path: String,
    expected_size: u64,
}

fn validate_audio_payload(data_len: usize) -> Result<()> {
    if data_len > MAX_AUDIO_PAYLOAD {
        return Err(Error::InvalidArgument(format!(
            "audio payload {data_len} exceeds {MAX_AUDIO_PAYLOAD} bytes"
        )));
    }
    Ok(())
}

fn validate_whisper_targets(clients: &[u16], channels: &[u64]) -> Result<()> {
    if clients.len() > MAX_WHISPER_CLIENTS {
        return Err(Error::InvalidArgument(format!(
            "whisper clients {} exceeds {MAX_WHISPER_CLIENTS}",
            clients.len()
        )));
    }
    if channels.len() > MAX_WHISPER_CHANNELS {
        return Err(Error::InvalidArgument(format!(
            "whisper channels {} exceeds {MAX_WHISPER_CHANNELS}",
            channels.len()
        )));
    }
    Ok(())
}

/// Compute the server UID using the algorithm of its version family.
fn server_uid(server: &tsclientlib::data::Server) -> String {
    let major = server
        .version
        .split('.')
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    if major >= 6 {
        let key = server.public_key.to_ts();
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(key.as_bytes()))
    } else {
        server.public_key.get_uid()
    }
}

/// Encode a channel password for file commands. Empty when absent.
fn encode_channel_password(password: Option<&str>) -> String {
    password
        .map(|password| tsproto_types::crypto::encode_password(password.as_bytes()))
        .unwrap_or_default()
}

/// Read exactly `size` bytes, resetting the idle timer after every chunk.
async fn read_exact_with_idle_timeout<R: AsyncRead + Unpin>(
    reader: &mut R,
    size: u64,
    config: &FileTransferConfig,
) -> std::result::Result<Vec<u8>, String> {
    let size = usize::try_from(size).map_err(|_| format!("file too large: {size} bytes"))?;
    let mut data = Vec::with_capacity(size.min(1024 * 1024));
    let mut buffer = vec![0u8; config.chunk_size.max(1)];

    while data.len() < size {
        let remaining = size - data.len();
        let want = remaining.min(buffer.len());
        let read = match config.idle_timeout {
            Some(timeout) => tokio::time::timeout(timeout, reader.read(&mut buffer[..want]))
                .await
                .map_err(|_| format!("file transfer idle timeout after {} ms", timeout.as_millis()))?,
            None => reader.read(&mut buffer[..want]).await,
        }
        .map_err(|e| format!("read failed: {e}"))?;

        if read == 0 {
            return Err(format!(
                "unexpected end of file: got {} of {} bytes",
                data.len(),
                size
            ));
        }
        data.extend_from_slice(&buffer[..read]);
    }

    Ok(data)
}

/// Write all bytes, resetting the idle timer after every chunk.
async fn write_all_with_idle_timeout<W: AsyncWrite + Unpin>(
    writer: &mut W,
    data: &[u8],
    config: &FileTransferConfig,
) -> std::result::Result<(), String> {
    let chunk_size = config.chunk_size.max(1);
    let mut written = 0;

    while written < data.len() {
        let end = (written + chunk_size).min(data.len());
        let write = writer.write_all(&data[written..end]);
        match config.idle_timeout {
            Some(timeout) => tokio::time::timeout(timeout, write)
                .await
                .map_err(|_| format!("file transfer idle timeout after {} ms", timeout.as_millis()))?,
            None => write.await,
        }
        .map_err(|e| format!("write failed: {e}"))?;
        written = end;
    }

    Ok(())
}

/// Copy exactly `size` bytes from a TCP stream into a local file.
async fn copy_stream_to_file<R: AsyncRead + Unpin>(
    stream: &mut R,
    size: u64,
    destination: &Path,
    offset: u64,
    config: &FileTransferConfig,
) -> std::result::Result<u64, String> {
    use tokio::fs::OpenOptions;

    let mut file = if offset == 0 {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(destination)
            .await
    } else {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(destination)
            .await
    }
    .map_err(|e| format!("open {} failed: {e}", destination.display()))?;

    let chunk = config.chunk_size.max(1);
    let mut buffer = vec![0u8; chunk];
    let mut remaining = size;
    let mut written = 0u64;

    while remaining > 0 {
        let want = remaining.min(chunk as u64) as usize;
        let read = match config.idle_timeout {
            Some(timeout) => tokio::time::timeout(timeout, stream.read(&mut buffer[..want]))
                .await
                .map_err(|_| {
                    format!(
                        "file transfer idle timeout after {} ms",
                        timeout.as_millis()
                    )
                })?,
            None => stream.read(&mut buffer[..want]).await,
        }
        .map_err(|e| format!("read failed: {e}"))?;

        if read == 0 {
            return Err(format!(
                "unexpected end of file: got {written} of {size} bytes"
            ));
        }
        file.write_all(&buffer[..read])
            .await
            .map_err(|e| format!("write {} failed: {e}", destination.display()))?;
        remaining -= read as u64;
        written += read as u64;
    }

    file.flush()
        .await
        .map_err(|e| format!("flush {} failed: {e}", destination.display()))?;
    Ok(written)
}

/// Copy a local file (starting at `offset`) into a TCP stream.
async fn copy_file_to_stream<W: AsyncWrite + Unpin>(
    source: &Path,
    offset: u64,
    size: u64,
    stream: &mut W,
    config: &FileTransferConfig,
) -> std::result::Result<(), String> {
    let mut file = tokio::fs::File::open(source)
        .await
        .map_err(|e| format!("open {} failed: {e}", source.display()))?;
    if offset > 0 {
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| format!("seek {} failed: {e}", source.display()))?;
    }

    let chunk = config.chunk_size.max(1);
    let mut buffer = vec![0u8; chunk];
    let mut remaining = size.saturating_sub(offset);

    while remaining > 0 {
        let want = remaining.min(chunk as u64) as usize;
        let read = file
            .read(&mut buffer[..want])
            .await
            .map_err(|e| format!("read {} failed: {e}", source.display()))?;
        if read == 0 {
            return Err(format!(
                "upload source ended early: {} bytes still expected",
                remaining
            ));
        }

        let write = stream.write_all(&buffer[..read]);
        match config.idle_timeout {
            Some(timeout) => tokio::time::timeout(timeout, write)
                .await
                .map_err(|_| {
                    format!(
                        "file transfer idle timeout after {} ms",
                        timeout.as_millis()
                    )
                })?,
            None => write.await,
        }
        .map_err(|e| format!("write failed: {e}"))?;
        remaining -= read as u64;
    }

    Ok(())
}

pub struct Client {
    /// The tsclientlib connection
    connection: Option<TsConnection>,
    /// Configuration
    config: ClientConfig,
    /// File transfer limits and timeouts
    file_transfer: FileTransferConfig,
    /// Current connection state
    state: ConnectionState,
    /// Server state (channels, users, etc.)
    server_state: ServerState,
    /// Event broadcaster
    event_tx: broadcast::Sender<Event>,
    /// Event handlers
    handlers: Vec<Arc<dyn EventHandler>>,
    /// Our client ID on the server
    client_id: Option<u16>,
    /// Current channel ID
    channel_id: Option<u64>,
    /// Audio packet sequence number
    audio_sequence: u16,
    /// Track which users are currently talking (user_id -> talk timeout deadline)
    talking_users: HashMap<u16, Instant>,
    /// Ordered talk deadlines; lets the timeout check stop as soon as the
    /// earliest live deadline is in the future instead of scanning every
    /// talking user on each event pump (PR-03).
    talk_deadlines: BTreeSet<(Instant, u16)>,
    /// Pending file downloads: FiletransferHandle id -> (channel_id, path)
    pending_downloads: HashMap<u16, PendingDownload>,
    /// Pending file uploads: FiletransferHandle id -> upload state
    pending_uploads: HashMap<u16, PendingUpload>,
    /// Pending upload size verifications: return code -> request state
    pending_file_infos: HashMap<u16, PendingFileInfo>,
    /// Pending public file-info queries: return code -> request state
    pending_file_info_queries: HashMap<u16, PendingFileInfoQuery>,
    /// Local override for our own input muted state.
    /// tsclientlib's state book doesn't reflect our clientupdate commands
    /// (the server doesn't echo them back), so we track it separately.
    self_input_muted: Option<bool>,
    /// Pending file list requests: return code -> request state
    pending_file_lists: HashMap<u16, PendingFileList>,
    /// Pending permoverview requests: return code -> request state
    pending_perm_overviews: HashMap<u16, PendingPermOverview>,
    /// Stream items received during `wait_connected` that were not the
    /// BookEvents it waits for. They are processed by the next event pump
    /// call so messages/audio from the connection handshake are not lost.
    pending_stream_items: VecDeque<StreamItem>,
    /// Active whisper targets. When non-empty, `send_audio` routes voice as
    /// `C2SWhisper` to these targets instead of the current channel (TS3
    /// semantics: whispering and channel talk are mutually exclusive).
    whisper_clients: Vec<u16>,
    whisper_channels: Vec<u64>,
    /// Low-latency audio sink: when set, every received voice frame is
    /// delivered synchronously on the pump thread (sender id, sender packet
    /// sequence, payload, whisper flag) instead of being emitted
    /// as an `AudioReceived` event (which waits for the next event pull).
    audio_sink: Option<Arc<dyn Fn(u16, u16, &[u8], bool) + Send + Sync>>,
    /// Cached user snapshot for repeated UI polling
    users_snapshot: Option<Arc<[User]>>,
    /// Cached channel snapshot for repeated UI polling
    channels_snapshot: Option<Arc<[Channel]>>,
    /// When the current temporary disconnect started
    reconnect_started: Option<Instant>,
    /// Whether `ConnectionStalled` was already emitted for this disconnect
    reconnect_stalled_reported: bool,
}

impl Client {
    /// Connect to a TeamSpeak server
    pub fn connect(config: ClientConfig) -> Result<Self> {
        info!("Connecting to {}", config.address);

        // Create event broadcaster
        let (event_tx, _) = broadcast::channel(256);

        let mut client = Self {
            connection: None,
            config: config.clone(),
            file_transfer: FileTransferConfig::default(),
            state: ConnectionState::Disconnected,
            server_state: ServerState::default(),
            event_tx,
            handlers: Vec::new(),
            client_id: None,
            channel_id: None,
            audio_sequence: 0,
            talking_users: HashMap::new(),
            talk_deadlines: BTreeSet::new(),
            pending_downloads: HashMap::new(),
            pending_uploads: HashMap::new(),
            pending_file_infos: HashMap::new(),
            pending_file_info_queries: HashMap::new(),
            self_input_muted: None,
            pending_file_lists: HashMap::new(),
            pending_perm_overviews: HashMap::new(),
            pending_stream_items: VecDeque::new(),
            whisper_clients: Vec::new(),
            whisper_channels: Vec::new(),
            audio_sink: None,
            users_snapshot: None,
            channels_snapshot: None,
            reconnect_started: None,
            reconnect_stalled_reported: false,
        };

        // Establish connection
        client.do_connect()?;

        Ok(client)
    }

    /// Perform the actual connection to the server
    fn do_connect(&mut self) -> Result<()> {
        self.state = ConnectionState::Connecting;

        // Build connection options using the new API
        let mut options = TsConnection::build(self.config.address.clone())
            .name(self.config.nickname.clone())
            .identity(self.config.identity.to_ts_identity());

        // Add password if specified
        if let Some(ref password) = self.config.password {
            options = options.password(password.clone());
        }

        // Add channel if specified
        if let Some(ref channel) = self.config.channel {
            options = options.channel(channel.clone());
        }

        // Add channel password if specified
        if let Some(ref channel_password) = self.config.channel_password {
            options = options.channel_password(channel_password.clone());
        }

        // Send the configured hardware id instead of the dependency default.
        options = options.hardware_id(self.config.hardware_id.clone());

        // Present a specific client version when configured.
        if let Some(ref version) = self.config.version {
            options = options.version(version.clone());
        }

        // Connect
        let con = options
            .connect()
            .map_err(|e| ConnectionError::ConnectFailed(e.to_string()))?;

        self.connection = Some(con);
        self.state = ConnectionState::Connected;

        info!("Connected to {}", self.config.address);

        // The user-visible `Connected` event is emitted by `wait_connected`
        // after the server's initial state has been synchronized.
        Ok(())
    }

    /// Process pending events from the server
    ///
    /// This method must be called regularly to receive server events.
    /// Returns a vector of events that occurred.
    pub async fn process_events(&mut self) -> Result<Vec<Event>> {
        if self.connection.is_none() {
            return Err(ConnectionError::NotConnected.into());
        }
        let mut events = Vec::new();
        let mut stream_items: Vec<StreamItem> = self.pending_stream_items.drain(..).collect();
        let mut disconnected = false;

        // First, collect all pending stream items
        {
            let con = self
                .connection
                .as_mut()
                .ok_or(ConnectionError::NotConnected)?;

            // Process events with a short timeout
            let timeout = tokio::time::Duration::from_millis(10);

            loop {
                match tokio::time::timeout(timeout, con.events().next()).await {
                    Ok(Some(Ok(item))) => {
                        stream_items.push(item);
                    }
                    Ok(Some(Err(e))) => {
                        warn!("Event stream error: {}", e);
                        break;
                    }
                    Ok(None) => {
                        // Stream ended, connection closed
                        disconnected = true;
                        break;
                    }
                    Err(_) => {
                        // Timeout, no more events for now
                        break;
                    }
                }
            }
        }

        // Now process the collected items
        let mut saw_book_events = false;
        for item in stream_items {
            if matches!(item, StreamItem::BookEvents(_)) {
                saw_book_events = true;
            }
            events.extend(self.process_stream_item(item).await);
        }

        if saw_book_events && self.state == ConnectionState::Reconnecting {
            if let Some(event) = self.restore_after_reconnect() {
                events.push(event);
            }
        }

        if disconnected {
            self.state = ConnectionState::Disconnected;
            let event = Event::Disconnected {
                reason: "Connection closed".to_string(),
            };
            let _ = self.event_tx.send(event.clone());
            events.push(event);
        }

        // Check for talk status timeouts
        let talk_stop_events = self.check_talk_timeouts();
        events.extend(talk_stop_events);

        // Dispatch events to handlers
        for event in &events {
            self.dispatch_event(event.clone()).await;
        }

        if let Some(event) = self.reconnect_watchdog_event() {
            events.push(event);
        }

        self.invalidate_snapshots();
        Ok(events)
    }

    /// Block until at least one event is available or `timeout_ms` elapses.
    ///
    /// Latency-critical pump for the connection stream: the first item wakes
    /// instantly on arrival (no polling interval), further items are only
    /// coalesced for a short grace window so a burst returns in one batch.
    /// This replaces sleep-then-poll loops on the embedding side.
    pub async fn wait_events(&mut self, timeout_ms: u64) -> Result<Vec<Event>> {
        if self.connection.is_none() {
            return Err(ConnectionError::NotConnected.into());
        }
        let mut events = Vec::new();
        let mut stream_items: Vec<StreamItem> = self.pending_stream_items.drain(..).collect();
        let mut disconnected = false;

        {
            let con = self
                .connection
                .as_mut()
                .ok_or(ConnectionError::NotConnected)?;

            let mut timeout = tokio::time::Duration::from_millis(timeout_ms.max(1));
            loop {
                match tokio::time::timeout(timeout, con.events().next()).await {
                    Ok(Some(Ok(item))) => {
                        stream_items.push(item);
                        timeout = tokio::time::Duration::from_millis(EVENT_BURST_GRACE_MS);
                        if stream_items.len() >= MAX_STREAM_BURST {
                            break;
                        }
                    }
                    Ok(Some(Err(e))) => {
                        warn!("Event stream error: {}", e);
                        break;
                    }
                    Ok(None) => {
                        // Stream ended, connection closed
                        disconnected = true;
                        break;
                    }
                    Err(_) => break, // timed out — return what we have
                }
            }
        }

        let mut saw_book_events = false;
        for item in stream_items {
            if matches!(item, StreamItem::BookEvents(_)) {
                saw_book_events = true;
            }
            events.extend(self.process_stream_item(item).await);
        }

        if saw_book_events && self.state == ConnectionState::Reconnecting {
            if let Some(event) = self.restore_after_reconnect() {
                events.push(event);
            }
        }

        if disconnected {
            self.state = ConnectionState::Disconnected;
            let event = Event::Disconnected {
                reason: "Connection closed".to_string(),
            };
            let _ = self.event_tx.send(event.clone());
            events.push(event);
        }

        let talk_stop_events = self.check_talk_timeouts();
        events.extend(talk_stop_events);

        for event in &events {
            self.dispatch_event(event.clone()).await;
        }

        if let Some(event) = self.reconnect_watchdog_event() {
            events.push(event);
        }

        self.invalidate_snapshots();
        Ok(events)
    }

    /// Register the low-latency audio sink. Pass `None` to restore the
    /// default `AudioReceived` event delivery. The closure receives
    /// `(sender_id, sender_packet_seq, data, is_whisper)`; the sequence is
    /// the sender's per-packet counter from the TS voice header, which
    /// receivers use to detect lost frames.
    pub fn set_audio_sink(
        &mut self,
        sink: Option<Arc<dyn Fn(u16, u16, &[u8], bool) + Send + Sync>>,
    ) {
        self.audio_sink = sink;
    }

    /// Replace the file transfer limits and timeouts.
    pub fn set_file_transfer_config(&mut self, config: FileTransferConfig) {
        self.file_transfer = config;
    }

    /// Current file transfer limits and timeouts.
    pub fn file_transfer_config(&self) -> &FileTransferConfig {
        &self.file_transfer
    }

    /// Check for users who have stopped talking (no audio received recently)
    fn check_talk_timeouts(&mut self) -> Vec<Event> {
        let now = Instant::now();
        let mut events = Vec::new();

        // Deadlines are ordered, so an idle server stops at the first live
        // deadline instead of scanning every tracked user.
        while let Some(&(deadline, user_id)) = self.talk_deadlines.iter().next() {
            if deadline > now {
                break;
            }
            self.talk_deadlines.remove(&(deadline, user_id));

            // Stale entry: the user sent more audio after this deadline.
            if self.talking_users.get(&user_id) != Some(&deadline) {
                continue;
            }
            self.talking_users.remove(&user_id);

            debug!("User {} stopped talking", user_id);
            if let Some(user) = self.server_state.users.get_mut(&user_id) {
                user.is_talking = false;
            }
            let event = Event::TalkStatusStop { user_id };
            let _ = self.event_tx.send(event.clone());
            events.push(event);
        }

        events
    }

    /// Emit a one-shot `ConnectionStalled` when a temporary disconnect has not
    /// recovered within `reconnect_watchdog`.
    ///
    /// This is a B-side mitigation for the dependency's receive-window
    /// behaviour: it does not change the connection state, so a late recovery
    /// can still complete normally.
    fn reconnect_watchdog_event(&mut self) -> Option<Event> {
        if self.state != ConnectionState::Reconnecting || self.reconnect_stalled_reported {
            return None;
        }
        let timeout = self.config.reconnect_watchdog?;
        let started = self.reconnect_started?;
        let elapsed = started.elapsed();
        if elapsed < timeout {
            return None;
        }

        self.reconnect_stalled_reported = true;
        let event = Event::ConnectionStalled {
            silent_for_ms: elapsed.as_millis() as u64,
        };
        warn!(
            "Reconnect watchdog: still reconnecting after {} ms",
            elapsed.as_millis()
        );
        let _ = self.event_tx.send(event.clone());
        Some(event)
    }

    /// Restore session-scoped state after the dependency reconnects.
    fn restore_after_reconnect(&mut self) -> Option<Event> {
        self.state = ConnectionState::Connected;
        self.reconnect_started = None;
        self.reconnect_stalled_reported = false;

        if let Err(e) = self.subscribe_all_channels() {
            warn!("Reconnect: failed to resubscribe channels: {}", e);
        }
        if let Err(e) = self.sync_state() {
            warn!("Reconnect: failed to sync state: {}", e);
        }
        if let Err(e) = self.send_server_variables() {
            warn!("Reconnect: failed to request server variables: {}", e);
        }
        if let Some(muted) = self.self_input_muted {
            if let Err(e) = self.set_input_muted(muted) {
                warn!("Reconnect: failed to restore input mute: {}", e);
            }
        }

        // Client/channel ids from the previous session are invalid.
        self.whisper_clients.clear();
        self.whisper_channels.clear();

        let event = Event::Reconnected {
            server_name: self.server_state.server.name.clone(),
            welcome_message: self.server_state.server.welcome_message.clone(),
        };
        let _ = self.event_tx.send(event.clone());
        info!(
            "Reconnected to {} - {} users, {} channels",
            self.server_state.server.name,
            self.server_state.users.len(),
            self.server_state.channels.len()
        );
        Some(event)
    }

    /// Wait for the initial connection to be established
    ///
    /// This waits until we receive the first BookEvents, indicating
    /// the connection is fully established. It also automatically
    /// subscribes to all channels and synchronizes the server state
    /// (users, channels, etc.).
    pub async fn wait_connected(&mut self) -> Result<()> {
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;

        // Hard bound: the embedding drives this on its single native thread —
        // an unresponsive server must not pin that thread forever.
        let wait_timeout = self
            .config
            .connect_timeout
            .max(std::time::Duration::from_secs(1));
        let deadline = tokio::time::Instant::now() + wait_timeout;

        // Poll events one by one without filtering, so we don't
        // discard non-BookEvents items (messages, audio, etc.)
        let mut deferred = Vec::new();
        loop {
            match tokio::time::timeout_at(deadline, con.events().next()).await {
                Ok(Some(Ok(item))) => {
                    if matches!(item, StreamItem::BookEvents(_)) {
                        break;
                    }
                    // Keep other events for the next event pump call instead
                    // of dropping messages/audio from the handshake.
                    deferred.push(item);
                }
                Ok(Some(Err(e))) => {
                    return Err(ConnectionError::ConnectFailed(e.to_string()).into());
                }
                Ok(None) => {
                    return Err(
                        ConnectionError::ConnectFailed("Connection closed".to_string()).into(),
                    );
                }
                Err(_elapsed) => {
                    return Err(ConnectionError::ConnectFailed(
                        "connect wait timed out".to_string(),
                    )
                    .into());
                }
            }
        }

        // Update connection state
        self.state = ConnectionState::Connected;

        // Preserve handshake-time events for the next process_events/wait_events
        // call. Keep the queue bounded so a misbehaving server cannot grow it
        // without limit.
        const MAX_PENDING_STREAM_ITEMS: usize = 256;
        for item in deferred {
            if self.pending_stream_items.len() >= MAX_PENDING_STREAM_ITEMS {
                self.pending_stream_items.pop_front();
            }
            self.pending_stream_items.push_back(item);
        }

        // Subscribe to all channels so we can see all users
        self.subscribe_all_channels()?;

        // Synchronize full state from server
        self.sync_state()?;

        // Ask the server to push current variable values (client/channel
        // counts, uptime) via notifyserverupdated. Not critical — a failure
        // only means those values stay at their defaults.
        if let Err(e) = self.send_server_variables() {
            warn!("Failed to request server variables: {}", e);
        }

        // Emit connected event
        let _ = self.event_tx.send(Event::Connected {
            server_name: self.server_state.server.name.clone(),
            welcome_message: self.server_state.server.welcome_message.clone(),
        });

        info!(
            "Connected to {} - {} users, {} channels",
            self.server_state.server.name,
            self.server_state.users.len(),
            self.server_state.channels.len()
        );

        // Default to input muted (PTT mode) so other clients see the mute icon.
        // Users who want an open mic can set `initial_input_muted(false)`.
        if self.config.initial_input_muted {
            if let Err(e) = self.set_input_muted(true) {
                warn!("Failed to set initial input muted state: {}", e);
            }
        }

        Ok(())
    }

    /// Subscribe to all channels on the server
    ///
    /// This makes all users visible regardless of which channel they are in.
    pub fn subscribe_all_channels(&mut self) -> Result<()> {
        self.ensure_connected()?;
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;

        let cmd = OutCommand::new(
            Direction::C2S,
            Flags::empty(),
            PacketType::Command,
            "channelsubscribeall",
        );

        cmd.send(con)
            .map_err(|e| Error::Internal(e.to_string()))?;

        debug!("Subscribed to all channels");
        Ok(())
    }

    /// Request the current server variables
    ///
    /// The server answers with a `notifyserverupdated`, which tsclientlib
    /// stores in `state.server.optional_data` (clients_online,
    /// channels_online, uptime). Without this request those values are
    /// never populated.
    pub fn send_server_variables(&mut self) -> Result<()> {
        self.ensure_connected()?;
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;

        let cmd = OutCommand::new(
            Direction::C2S,
            Flags::empty(),
            PacketType::Command,
            "servergetvariables",
        );

        cmd.send(con).map_err(|e| Error::Internal(e.to_string()))?;

        debug!("Requested server variables");
        Ok(())
    }

    /// Process a stream item from tsclientlib
    ///
    /// Returns all events generated by this stream item (may be multiple,
    /// e.g. TalkStatusStart + AudioReceived, or multiple BookEvents).
    async fn process_stream_item(&mut self, item: StreamItem) -> Vec<Event> {
        match item {
            StreamItem::BookEvents(book_events) => {
                // Process book events (client/channel changes)
                let mut events = Vec::new();
                for event in book_events {
                    if let Some(evt) = self.process_book_event(event) {
                        // Send via broadcast channel
                        let _ = self.event_tx.send(evt.clone());
                        events.push(evt);
                    }
                }
                events
            }
            StreamItem::Audio(audio) => {
                // Audio received - extract data from InAudioBuf
                let audio_data = audio.data().data();
                let (from_id, seq, codec, data, is_whisper) = match audio_data {
                    AudioData::S2C { id, from, codec, data } => (*from, *id, *codec, data, false),
                    AudioData::S2CWhisper { id, from, codec, data } => (*from, *id, *codec, data, true),
                    _ => return Vec::new(), // C2S packets should not be received
                };

                let mut events = Vec::new();

                // Check if this is a new talk session (user wasn't talking before)
                let was_talking = self.talking_users.contains_key(&from_id);
                let deadline = Instant::now() + Duration::from_millis(TALK_TIMEOUT_MS as u64);
                if let Some(previous) = self.talking_users.insert(from_id, deadline) {
                    self.talk_deadlines.remove(&(previous, from_id));
                }
                self.talk_deadlines.insert((deadline, from_id));

                if !was_talking {
                    // Emit TalkStatusStart — both via broadcast AND in returned events
                    if let Some(user) = self.server_state.users.get_mut(&from_id) {
                        user.is_talking = true;
                    }
                    let start_event = Event::TalkStatusStart { user_id: from_id, is_whisper };
                    let _ = self.event_tx.send(start_event.clone());
                    events.push(start_event);
                    debug!("User {} started talking (whisper: {})", from_id, is_whisper);
                }

                // Hot path: deliver to the registered sink synchronously (no
                // event round-trip); fall back to the event stream otherwise
                if let Some(sink) = self.audio_sink.as_ref() {
                    sink(from_id, seq, data, is_whisper);
                } else {
                    let event = Event::AudioReceived {
                        user_id: from_id,
                        codec: codec_type_to_audio_codec(codec),
                        data: Arc::from(*data),
                        seq,
                        is_whisper,
                    };
                    let _ = self.event_tx.send(event.clone());
                    events.push(event);
                }
                events
            }
            StreamItem::DisconnectedTemporarily(_reason) => {
                self.state = ConnectionState::Reconnecting;
                self.reconnect_started = Some(Instant::now());
                self.reconnect_stalled_reported = false;
                let event = Event::ConnectionLost {
                    reason: "Temporary disconnect".to_string(),
                };
                let _ = self.event_tx.send(event.clone());
                vec![event]
            }
            StreamItem::MessageEvent(msg) => {
                debug!("StreamItem::MessageEvent: cmd={:?}", msg.get_command_name());
                self.process_message_event(msg)
            }
            StreamItem::FileDownload(handle, mut result) => {
                let pending = self
                    .pending_downloads
                    .remove(&handle.0)
                    .unwrap_or(PendingDownload {
                        channel_id: 0,
                        path: String::new(),
                        destination: None,
                        offset: 0,
                    });
                let PendingDownload {
                    channel_id,
                    path,
                    destination,
                    offset,
                } = pending;

                let size = result.size;
                if destination.is_none() {
                    if let Some(limit) = self.file_transfer.max_memory_download {
                        if size > limit {
                            let error = format!("file too large: {size} bytes (limit {limit})");
                            warn!("File download rejected: path={}, {error}", path);
                            let event = Event::FileTransferFailed {
                                path: path.clone(),
                                error,
                            };
                            let _ = self.event_tx.send(event.clone());
                            return vec![event];
                        }
                    }
                }

                info!(
                    "FileDownload received: path={}, size={}, destination={:?}",
                    path, size, destination
                );

                if let Some(destination) = destination {
                    let remaining = size.saturating_sub(offset);
                    match copy_stream_to_file(
                        &mut result.stream,
                        remaining,
                        &destination,
                        offset,
                        &self.file_transfer,
                    )
                    .await
                    {
                        Ok(written) => {
                            info!(
                                "File downloaded to path: {} -> {} ({} bytes)",
                                path,
                                destination.display(),
                                written
                            );
                            let event = Event::FileDownloadedToPath {
                                channel_id,
                                path,
                                destination: destination.to_string_lossy().into_owned(),
                                size: written,
                            };
                            let _ = self.event_tx.send(event.clone());
                            vec![event]
                        }
                        Err(error) => {
                            warn!("File download failed: path={}, {error}", path);
                            let event = Event::FileTransferFailed { path, error };
                            let _ = self.event_tx.send(event.clone());
                            vec![event]
                        }
                    }
                } else {
                    match read_exact_with_idle_timeout(
                        &mut result.stream,
                        size,
                        &self.file_transfer,
                    )
                    .await
                    {
                        Ok(data) => {
                            info!("File downloaded: path={}, size={}", path, data.len());
                            let event = Event::FileDownloaded {
                                channel_id,
                                path,
                                data,
                            };
                            let _ = self.event_tx.send(event.clone());
                            vec![event]
                        }
                        Err(error) => {
                            warn!("File download failed: path={}, {error}", path);
                            let event = Event::FileTransferFailed { path, error };
                            let _ = self.event_tx.send(event.clone());
                            vec![event]
                        }
                    }
                }
            }
            StreamItem::FileUpload(handle, mut result) => {
                if let Some(upload) = self.pending_uploads.remove(&handle.0) {
                    let PendingUpload {
                        channel_id,
                        path,
                        size,
                        source,
                        password,
                    } = upload;
                    let offset = result.seek_position;
                    if offset > size {
                        let error = format!(
                            "server resume offset {offset} exceeds upload size {size}"
                        );
                        warn!("Upload rejected: path={}, {error}", path);
                        let event = Event::FileTransferFailed { path, error };
                        let _ = self.event_tx.send(event.clone());
                        return vec![event];
                    }

                    let written = match source {
                        UploadSource::Memory(data) => {
                            let to_write = &data[offset as usize..];
                            write_all_with_idle_timeout(
                                &mut result.stream,
                                to_write,
                                &self.file_transfer,
                            )
                            .await
                        }
                        UploadSource::File(source_path) => {
                            copy_file_to_stream(
                                &source_path,
                                offset,
                                size,
                                &mut result.stream,
                                &self.file_transfer,
                            )
                            .await
                        }
                    };

                    match written {
                        Ok(()) => {
                            info!("Upload complete: {} ({} bytes)", path, size);
                            // Ask the server to confirm the size it persisted.
                            // The FileUploaded event is emitted when the
                            // ftgetfileinfo response arrives, or immediately if
                            // verification cannot be requested.
                            self.verify_uploaded_file(
                                channel_id,
                                &path,
                                size,
                                password.as_deref(),
                            )
                        }
                        Err(error) => {
                            warn!("Upload failed: {} — {}", path, error);
                            let event = Event::FileTransferFailed { path, error };
                            let _ = self.event_tx.send(event.clone());
                            vec![event]
                        }
                    }
                } else {
                    Vec::new()
                }
            }
            StreamItem::FiletransferFailed(handle, error) => {
                // Check downloads first, then uploads
                let (_channel_id, path) = self
                    .pending_downloads
                    .remove(&handle.0)
                    .map(|download| (download.channel_id, download.path))
                    .or_else(|| {
                        self.pending_uploads
                            .remove(&handle.0)
                            .map(|upload| (upload.channel_id, upload.path))
                    })
                    .unwrap_or((0, String::new()));

                warn!("File transfer failed: path={}, error={}", path, error);
                let event = Event::FileTransferFailed {
                    path,
                    error: error.to_string(),
                };
                let _ = self.event_tx.send(event.clone());
                vec![event]
            }
            other => {
                debug!("Unhandled StreamItem variant: {:?}", std::mem::discriminant(&other));
                Vec::new()
            }
        }
    }

    /// Process a book event and return any high-level events to emit
    fn process_book_event(&mut self, event: TsEvent) -> Option<Event> {
        match &event {
            TsEvent::PropertyAdded { id, .. } => {
                debug!("Property added: {:?}", id);
                self.handle_property_added(id)
            }
            TsEvent::PropertyChanged { id, old, .. } => {
                debug!("Property changed: {:?}", id);
                self.handle_property_changed(id, old)
            }
            TsEvent::PropertyRemoved { id, old, invoker, extra } => {
                debug!("Property removed: {:?}", id);
                self.handle_property_removed(id, old, invoker.as_ref(), extra.reason.clone())
            }
            TsEvent::Message { target, invoker, message } => {
                match target {
                    TsMessageTarget::Poke(_) => {
                        let event = Event::Poked {
                            poker_id: invoker.id.0,
                            poker_name: invoker.name.clone(),
                            message: message.clone(),
                        };
                        debug!("Poked by {} ({})", invoker.name, invoker.id.0);
                        Some(event)
                    }
                    _ => {
                        let (msg_target, target_id) = match target {
                            TsMessageTarget::Server => {
                                (crate::events::MessageTarget::Server, None)
                            }
                            TsMessageTarget::Channel => (
                                crate::events::MessageTarget::Channel,
                                self.server_state
                                    .users
                                    .get(&invoker.id.0)
                                    .map(|user| user.channel_id)
                                    .or(self.channel_id),
                            ),
                            TsMessageTarget::Client(id) => (
                                crate::events::MessageTarget::Private,
                                Some(id.0 as u64),
                            ),
                            TsMessageTarget::Poke(_) => unreachable!(),
                        };
                        let event = Event::TextMessage {
                            sender_id: invoker.id.0,
                            sender_name: invoker.name.clone(),
                            message: message.clone(),
                            target: msg_target,
                            target_id,
                        };
                        debug!(
                            "Text message from {} ({}): {}",
                            invoker.name, invoker.id.0, message
                        );
                        Some(event)
                    }
                }
            }
        }
    }

    /// Handle a property being added (new client, channel, etc.)
    fn handle_property_added(&mut self, id: &PropertyId) -> Option<Event> {
        let con = self.connection.as_ref()?;
        let state = con.get_state().ok()?;

        match id {
            PropertyId::Client(client_id) => {
                // New client connected
                if let Some(ts_client) = state.clients.get(client_id) {
                    let user = ts_client_to_user(ts_client);
                    self.server_state.users.insert(user.id, user.clone());
                    info!("User joined: {} ({})", user.nickname, user.id);
                    return Some(Event::UserJoined { user });
                }
            }
            PropertyId::Channel(channel_id) => {
                // New channel created
                if let Some(ts_channel) = state.channels.get(channel_id) {
                    let mut channel = ts_channel_to_channel(ts_channel);
                    // Preserve our computed permission_hints (tsclientlib doesn't track permoverview results)
                    if channel.permission_hints == 0 {
                        if let Some(existing) = self.server_state.channels.get(&channel.id) {
                            channel.permission_hints = existing.permission_hints;
                        }
                    }
                    self.server_state.channels.insert(channel.id, channel.clone());
                    info!("Channel created: {} ({})", channel.name, channel.id);
                    return Some(Event::ChannelCreated { channel });
                }
            }
            _ => {}
        }
        None
    }

    /// Handle a property being changed
    fn handle_property_changed(&mut self, id: &PropertyId, _old: &tsclientlib::events::PropertyValue) -> Option<Event> {
        let con = self.connection.as_ref()?;
        let state = con.get_state().ok()?;

        match id {
            PropertyId::ClientChannel(client_id) => {
                // Client moved to a different channel
                if let Some(ts_client) = state.clients.get(client_id) {
                    let mut user = ts_client_to_user(ts_client);
                    user.is_talking = self.talking_users.contains_key(&user.id);
                    let from_channel = self.server_state.users
                        .get(&user.id)
                        .map(|u| u.channel_id)
                        .unwrap_or(0);

                    self.server_state.users.insert(user.id, user.clone());

                    if Some(user.id) == self.client_id {
                        self.channel_id = Some(user.channel_id);
                    }

                    if from_channel != user.channel_id {
                        info!("User {} moved from channel {} to {}", user.nickname, from_channel, user.channel_id);
                        if Some(user.id) == self.client_id {
                            if let Some(channel) =
                                self.server_state.channels.get(&user.channel_id).cloned()
                            {
                                return Some(Event::ChannelJoined { channel });
                            }
                        }
                        return Some(Event::UserMoved {
                            user,
                            from_channel,
                            to_channel: ts_client.channel.0,
                        });
                    }
                }
            }
            PropertyId::ClientName(client_id) |
            PropertyId::ClientInputMuted(client_id) |
            PropertyId::ClientOutputMuted(client_id) |
            PropertyId::ClientAwayMessage(client_id) |
            PropertyId::ClientTalkPower(client_id) |
            PropertyId::ClientTalkPowerGranted(client_id) |
            PropertyId::ClientIsRecording(client_id) => {
                // Client property changed - update our state
                if let Some(ts_client) = state.clients.get(client_id) {
                    let mut user = ts_client_to_user(ts_client);
                    user.is_talking = self.talking_users.contains_key(&user.id);
                    self.server_state.users.insert(user.id, user.clone());
                    return Some(Event::UserUpdated { user });
                }
            }
            PropertyId::ChannelName(channel_id) |
            PropertyId::ChannelTopic(channel_id) |
            PropertyId::ChannelCodec(channel_id) |
            PropertyId::ChannelMaxClients(channel_id) |
            PropertyId::ChannelNeededTalkPower(channel_id) => {
                // Channel property changed
                if let Some(ts_channel) = state.channels.get(channel_id) {
                    let mut channel = ts_channel_to_channel(ts_channel);
                    // Preserve our computed permission_hints
                    if channel.permission_hints == 0 {
                        if let Some(existing) = self.server_state.channels.get(&channel.id) {
                            channel.permission_hints = existing.permission_hints;
                        }
                    }
                    self.server_state.channels.insert(channel.id, channel.clone());
                    return Some(Event::ChannelEdited { channel });
                }
            }
            _ => {}
        }
        None
    }

    /// Handle a property being removed (client left, channel deleted, etc.)
    fn handle_property_removed(
        &mut self,
        id: &PropertyId,
        _old: &tsclientlib::events::PropertyValue,
        invoker: Option<&tsproto_types::Invoker>,
        reason: Option<Reason>,
    ) -> Option<Event> {
        match id {
            PropertyId::Client(client_id) => {
                // Client disconnected, kicked, or banned.
                if let Some(user) = self.server_state.users.remove(&client_id.0) {
                    info!("User left: {} ({})", user.nickname, user.id);
                    let reason_text = reason
                        .map(|reason| format!("{reason:?}"))
                        .unwrap_or_else(|| "Disconnected".to_string());
                    let kicker = invoker_to_user(invoker, &self.server_state);

                    match reason {
                        Some(Reason::KickChannel) => {
                            if let Some(kicker) = kicker {
                                return Some(Event::UserKickedFromChannel {
                                    user,
                                    kicker,
                                    reason: reason_text,
                                });
                            }
                        }
                        Some(Reason::KickServer) => {
                            if let Some(kicker) = kicker {
                                return Some(Event::UserKickedFromServer {
                                    user,
                                    kicker,
                                    reason: reason_text,
                                });
                            }
                        }
                        Some(Reason::KickServerBan) => {
                            if let Some(banner) = kicker {
                                return Some(Event::UserBanned {
                                    user,
                                    banner,
                                    reason: reason_text,
                                    duration: None,
                                });
                            }
                        }
                        _ => {}
                    }

                    return Some(Event::UserLeft {
                        user,
                        reason: reason_text,
                    });
                }
            }
            PropertyId::Channel(channel_id) => {
                // Channel deleted
                if self.server_state.channels.remove(&channel_id.0).is_some() {
                    info!("Channel deleted: {}", channel_id.0);
                    return Some(Event::ChannelDeleted {
                        channel_id: channel_id.0,
                    });
                }
            }
            _ => {}
        }
        None
    }

    /// Synchronize the full state from tsclientlib
    /// Call this after connection to populate initial state
    pub fn sync_state(&mut self) -> Result<()> {
        let con = self.connection.as_ref().ok_or(ConnectionError::NotConnected)?;
        let state = con.get_state().map_err(|e| Error::Internal(e.to_string()))?;

        // Clear existing state
        self.server_state.users.clear();
        self.server_state.channels.clear();

        // Sync server info
        self.server_state.server.name = state.server.name.clone();
        self.server_state.server.welcome_message = Some(state.server.welcome_message.clone());
        self.server_state.server.platform = state.server.platform.clone();
        self.server_state.server.version = state.server.version.clone();
        self.server_state.server.max_clients = state.server.max_clients as u32;
        self.server_state.server.icon_id = state.server.icon.0 as i64;
        self.server_state.server.uid = server_uid(&state.server);
        self.server_state.server.host_banner_url = if state.server.hostbanner_url.is_empty() {
            None
        } else {
            Some(state.server.hostbanner_url.clone())
        };
        self.server_state.server.host_banner_gfx_url =
            if state.server.hostbanner_gfx_url.is_empty() {
                None
            } else {
                Some(state.server.hostbanner_gfx_url.clone())
            };

        // Sync all channels (preserve computed permission_hints)
        let old_hints: HashMap<u64, u64> = self.server_state.channels.iter()
            .filter(|(_, ch)| ch.permission_hints != 0)
            .map(|(&id, ch)| (id, ch.permission_hints))
            .collect();
        for (_, ts_channel) in &state.channels {
            let mut channel = ts_channel_to_channel(ts_channel);
            if channel.permission_hints == 0 {
                if let Some(&hints) = old_hints.get(&channel.id) {
                    channel.permission_hints = hints;
                }
            }
            self.server_state.channels.insert(channel.id, channel);
        }

        // Sync all clients
        for (_, ts_client) in &state.clients {
            let user = ts_client_to_user(ts_client);
            self.server_state.users.insert(user.id, user);
        }

        // Update our client ID
        self.client_id = Some(state.own_client.0);

        // Update our channel ID
        if let Some(our_client) = state.clients.get(&state.own_client) {
            self.channel_id = Some(our_client.channel.0);
        }

        info!(
            "State synced: {} users, {} channels",
            self.server_state.users.len(),
            self.server_state.channels.len()
        );

        self.invalidate_snapshots();
        Ok(())
    }

    /// Process an incoming message event (non-book protocol messages)
    ///
    /// Note: Text messages and pokes arrive via `BookEvents` as `Event::Message`,
    /// not here. This handles remaining protocol messages like channellistfinished.
    fn process_message_event(&mut self, msg: InMessage) -> Vec<Event> {
        debug!("process_message_event: cmd={}", msg.get_command_name());
        match msg {
            InMessage::FileList(file_list) => {
                let count = file_list.iter().count();
                debug!("Received FileList with {} entries", count);

                let key = file_list
                    .return_code
                    .as_ref()
                    .and_then(|code| code.parse::<u16>().ok());
                let (fallback_cid, fallback_path) = file_list
                    .iter()
                    .next()
                    .map(|part| (part.channel_id.0, part.path.clone()))
                    .unwrap_or((0, String::new()));
                let pending = match key {
                    Some(key) => self.pending_file_lists.get_mut(&key),
                    None => self
                        .pending_file_lists
                        .values_mut()
                        .find(|pending| {
                            pending.channel_id == fallback_cid && pending.path == fallback_path
                        }),
                };

                if let Some(pending) = pending {
                    for part in file_list.iter() {
                        debug!(
                            "  file entry: name={}, size={}, is_file={}",
                            part.name,
                            part.size,
                            part.is_file
                        );
                        pending.entries.push(FileEntry {
                            name: part.name.clone(),
                            size: part.size,
                            datetime: part.date_time.unix_timestamp(),
                            is_file: part.is_file,
                        });
                    }
                } else {
                    log::warn!("Received FileList but no pending request");
                }
                Vec::new()
            }
            InMessage::FileListFinished(finished) => {
                debug!("Received FileListFinished");
                let mut events = Vec::new();
                for part in finished.iter() {
                    let channel_id = part.channel_id.0;
                    let path = part.path.clone();
                    let key = self
                        .pending_file_lists
                        .iter()
                        .find(|(_, pending)| {
                            pending.channel_id == channel_id && pending.path == path
                        })
                        .map(|(key, _)| *key);

                    if let Some(key) = key {
                        if let Some(pending) = self.pending_file_lists.remove(&key) {
                            debug!(
                                "File list complete: channel={}, path={}, {} files",
                                pending.channel_id,
                                pending.path,
                                pending.entries.len()
                            );
                            let event = Event::FileListReceived {
                                channel_id: pending.channel_id,
                                path: pending.path,
                                files: pending.entries,
                            };
                            let _ = self.event_tx.send(event.clone());
                            events.push(event);
                        }
                    } else {
                        log::warn!(
                            "FileListFinished for unknown request: channel={}, path={}",
                            channel_id,
                            path
                        );
                    }
                }
                events
            }
            InMessage::PermOverview(ref perm) => {
                let key = perm
                    .return_code
                    .as_ref()
                    .and_then(|code| code.parse::<u16>().ok());
                let fallback_cid = perm
                    .iter()
                    .next()
                    .map(|part| part.channel_id.0)
                    .unwrap_or(0);
                let key = match key {
                    Some(key) if self.pending_perm_overviews.contains_key(&key) => Some(key),
                    _ if self.pending_perm_overviews.len() == 1 => {
                        self.pending_perm_overviews.keys().next().copied()
                    }
                    _ => self
                        .pending_perm_overviews
                        .iter()
                        .find(|(_, pending)| pending.channel_id == fallback_cid)
                        .map(|(key, _)| *key),
                };
                let Some(key) = key else {
                    log::warn!("permoverview response without a pending request");
                    return Vec::new();
                };
                let Some(mut pending) = self.pending_perm_overviews.remove(&key) else {
                    return Vec::new();
                };

                for part in perm.iter() {
                    let perm_id = part.permission_id.0;
                    let value = part.permission_value;
                    // Keep the best value: -1 (unlimited) wins, otherwise take max
                    let entry = pending.perms.entry(perm_id).or_insert(0);
                    if *entry != -1 && (value == -1 || value > *entry) {
                        *entry = value;
                    }
                }

                // Compute hints immediately — tsclientlib consumes the terminating
                // "error id=0" internally, so CommandError never fires for permoverview.
                let hints = compute_file_permission_hints(&pending.perms);
                debug!(
                    "Channel {} permission_hints computed: {:#x} ({} perms)",
                    pending.channel_id,
                    hints.bits,
                    pending.perms.len()
                );
                if let Some(ch) = self.server_state.channels.get_mut(&pending.channel_id) {
                    ch.permission_hints = hints.bits;
                    ch.permission_hints_known = hints.known;
                }
                let event = Event::ChannelPermissionsUpdated {
                    channel_id: pending.channel_id,
                    permission_hints: hints.bits,
                    known: hints.known,
                };
                let _ = self.event_tx.send(event.clone());
                vec![event]
            }
            InMessage::FileInfo(ref info) => {
                let key = info
                    .return_code
                    .as_ref()
                    .and_then(|code| code.parse::<u16>().ok());

                // Public `ftgetfileinfo` query (not upload verification).
                let query_key = match key {
                    Some(key) if self.pending_file_info_queries.contains_key(&key) => Some(key),
                    _ => info.iter().next().and_then(|part| {
                        self.pending_file_info_queries
                            .iter()
                            .find(|(_, pending)| {
                                pending.channel_id == part.channel_id.0
                                    && (pending.path == part.name
                                        || pending.path.ends_with(&format!("/{}", part.name)))
                            })
                            .map(|(key, _)| *key)
                    }),
                };
                if let Some(key) = query_key {
                    if let Some(query) = self.pending_file_info_queries.remove(&key) {
                        if let Some(part) = info.iter().next() {
                            let event = Event::FileInfoReceived {
                                channel_id: query.channel_id,
                                path: query.path,
                                size: part.size,
                                datetime: part.date_time.unix_timestamp(),
                                is_file: true,
                            };
                            let _ = self.event_tx.send(event.clone());
                            return vec![event];
                        }
                    }
                }

                let key = match key {
                    Some(key) if self.pending_file_infos.contains_key(&key) => Some(key),
                    _ => info.iter().next().and_then(|part| {
                        self.pending_file_infos
                            .iter()
                            .find(|(_, pending)| {
                                pending.channel_id == part.channel_id.0
                                    && (pending.path == part.name
                                        || pending.path.ends_with(&format!("/{}", part.name)))
                            })
                            .map(|(key, _)| *key)
                    }),
                };

                let Some(key) = key else {
                    log::warn!("fileinfo response without a pending upload verification");
                    return Vec::new();
                };
                let Some(pending) = self.pending_file_infos.remove(&key) else {
                    return Vec::new();
                };

                match info.iter().next().map(|part| part.size) {
                    Some(size) if size == pending.expected_size => {
                        debug!("Upload verified: {} ({} bytes)", pending.path, size);
                        let event = Event::FileUploaded {
                            channel_id: pending.channel_id,
                            path: pending.path,
                        };
                        let _ = self.event_tx.send(event.clone());
                        vec![event]
                    }
                    Some(size) => {
                        let error = Error::FileTransferCorrupted(format!(
                            "size mismatch for {}: expected {} bytes, server reports {}",
                            pending.path, pending.expected_size, size
                        ));
                        warn!("Upload verification failed: {}", error);
                        let event = Event::FileTransferFailed {
                            path: pending.path,
                            error: error.to_string(),
                        };
                        let _ = self.event_tx.send(event.clone());
                        vec![event]
                    }
                    None => {
                        warn!(
                            "Upload verification for {} returned no file entry",
                            pending.path
                        );
                        let event = Event::FileUploaded {
                            channel_id: pending.channel_id,
                            path: pending.path,
                        };
                        let _ = self.event_tx.send(event.clone());
                        vec![event]
                    }
                }
            }
            InMessage::Filetransfer(ref list) => {
                let transfers = list
                    .iter()
                    .map(|part| crate::events::FileTransferInfo {
                        client_id: part.client_id.0,
                        path: part.path.clone(),
                        name: part.name.clone(),
                        size: part.size,
                        size_done: part.size_done,
                        client_filetransfer_id: part.client_filetransfer_id,
                        server_filetransfer_id: part.server_filetransfer_id,
                        sender: part.sender,
                        status: part.status,
                        current_speed: part.current_speed,
                        average_speed: part.average_speed,
                        runtime_secs: part.runtime.whole_seconds().max(0) as u64,
                    })
                    .collect();
                let event = Event::FileTransfersListed { transfers };
                let _ = self.event_tx.send(event.clone());
                vec![event]
            }
            InMessage::FiletransferStatus(ref status) => {
                let mut events = Vec::new();
                for part in status.iter() {
                    let event = Event::FileTransferStopped {
                        client_filetransfer_id: part.client_filetransfer_id,
                        status: part.status as u32,
                        message: part.message.clone(),
                        size: part.size,
                    };
                    let _ = self.event_tx.send(event.clone());
                    events.push(event);
                }
                events
            }
            InMessage::CommandError(ref err) => {
                for part in err.iter() {
                    debug!("CommandError: id={:?}, msg={}", part.id, part.message);
                }

                let mut events = Vec::new();
                for part in err.iter() {
                    let error_id = part.id as u32;
                    let return_code = part
                        .return_code
                        .as_ref()
                        .and_then(|code| code.parse::<u16>().ok());

                    // "database empty result" (1281): empty directory instead
                    // of FileListFinished.
                    if part.id == TsError::DatabaseEmptyResult {
                        if let Some(key) = return_code {
                            if let Some(pending) = self.pending_file_lists.remove(&key) {
                                debug!(
                                    "File list empty dir: channel={}, path={}",
                                    pending.channel_id,
                                    pending.path
                                );
                                let event = Event::FileListReceived {
                                    channel_id: pending.channel_id,
                                    path: pending.path,
                                    files: pending.entries,
                                };
                                let _ = self.event_tx.send(event.clone());
                                events.push(event);
                            }
                        }
                    }

                    // Failed permoverview: the result is unknown, not "denied".
                    // Only resolve the request this error belongs to.
                    if error_id != 0 {
                        if let Some(key) = return_code {
                            if let Some(pending) = self.pending_perm_overviews.remove(&key) {
                                debug!(
                                    "permoverview failed for channel {}, hints unknown",
                                    pending.channel_id
                                );
                                let hints = PermissionHints::UNKNOWN;
                                if let Some(ch) =
                                    self.server_state.channels.get_mut(&pending.channel_id)
                                {
                                    ch.permission_hints = hints.bits;
                                    ch.permission_hints_known = hints.known;
                                }
                                let event = Event::ChannelPermissionsUpdated {
                                    channel_id: pending.channel_id,
                                    permission_hints: hints.bits,
                                    known: hints.known,
                                };
                                let _ = self.event_tx.send(event.clone());
                                events.push(event);
                            }
                        }

                        // Failed upload verification. The transfer itself
                        // already completed, so only missing browse permission
                        // (2568) is tolerated; other server errors mean the
                        // file could not be confirmed.
                        let mut verification_handled = false;
                        if let Some(key) = return_code {
                            if let Some(pending) = self.pending_file_infos.remove(&key) {
                                verification_handled = true;
                                if error_id == 2568 {
                                    warn!(
                                        "Upload verification skipped for {}: no file browse permission",
                                        pending.path
                                    );
                                    let event = Event::FileUploaded {
                                        channel_id: pending.channel_id,
                                        path: pending.path,
                                    };
                                    let _ = self.event_tx.send(event.clone());
                                    events.push(event);
                                } else {
                                    let error = Error::server(error_id, part.message.clone());
                                    warn!(
                                        "Upload verification failed for {}: {}",
                                        pending.path, error
                                    );
                                    let event = Event::FileTransferFailed {
                                        path: pending.path,
                                        error: error.to_string(),
                                    };
                                    let _ = self.event_tx.send(event.clone());
                                    events.push(event);
                                }
                            }
                        }

                        if !verification_handled {
                            let event = Event::CommandError {
                                error_id,
                                message: part.message.clone(),
                            };
                            let _ = self.event_tx.send(event.clone());
                            events.push(event);
                        }
                    }
                }
                events
            }
            other => {
                let command = other.get_command_name().to_string();
                debug!("Unhandled MessageEvent: {}", command);
                let event = Event::Notification { command };
                let _ = self.event_tx.send(event.clone());
                vec![event]
            }
        }
    }

    /// Dispatch an event to all handlers
    async fn dispatch_event(&self, event: Event) {
        for handler in &self.handlers {
            handler.on_event(&event).await;
        }
    }

    /// Add an event handler
    pub fn add_handler(&mut self, handler: Arc<dyn EventHandler>) {
        self.handlers.push(handler);
    }

    /// Get the current connection state
    pub fn state(&self) -> ConnectionState {
        self.state.clone()
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.state.is_connected()
    }

    /// Reject commands unless the client is in an active state. Commands must
    /// not be sent after `disconnect()` starts or while reconnecting.
    fn ensure_connected(&self) -> Result<()> {
        if self.state.is_connected() {
            Ok(())
        } else {
            Err(ConnectionError::NotConnected.into())
        }
    }

    /// Get our client ID
    pub fn client_id(&self) -> Option<u16> {
        self.client_id
    }

    /// Get our current channel ID
    pub fn channel_id(&self) -> Option<u64> {
        self.channel_id
    }

    /// Get a snapshot of the server state
    pub fn server_state(&self) -> &ServerState {
        &self.server_state
    }

    /// Build a ServerInfo snapshot from the live tsclientlib state
    ///
    /// `clients_online`, `channels_online` and `uptime` only exist in the
    /// book's `optional_data`, which the server pushes via
    /// `notifyserverupdated` after `send_server_variables()`. Falls back to
    /// the cached `server_state` when not connected.
    pub fn server_info(&self) -> ServerInfo {
        let mut info = self.server_state.server.clone();
        if let Some(con) = self.connection.as_ref() {
            if let Ok(state) = con.get_state() {
                let server = &state.server;
                info.name = server.name.clone();
                info.welcome_message = Some(server.welcome_message.clone());
                info.platform = server.platform.clone();
                info.version = server.version.clone();
                info.max_clients = server.max_clients as u32;
                info.icon_id = server.icon.0 as i64;
                info.uid = server_uid(server);
                info.host_banner_url = if server.hostbanner_url.is_empty() {
                    None
                } else {
                    Some(server.hostbanner_url.clone())
                };
                info.host_banner_gfx_url = if server.hostbanner_gfx_url.is_empty() {
                    None
                } else {
                    Some(server.hostbanner_gfx_url.clone())
                };
                if let Some(opt) = &server.optional_data {
                    info.clients_online = opt.client_count as u32;
                    info.channels_online = opt.channel_count as u32;
                    info.uptime = opt.uptime.whole_seconds().max(0) as u64;
                }
            }
        }
        info
    }

    /// Get all channels from tsclientlib state as a shared snapshot.
    ///
    /// The snapshot is rebuilt after an event pump or a state mutation, so
    /// repeated UI reads in one frame share a single allocation.
    pub fn channels(&mut self) -> Arc<[Channel]> {
        if let Some(snapshot) = &self.channels_snapshot {
            return Arc::clone(snapshot);
        }
        let snapshot: Arc<[Channel]> = self.build_channels().into();
        self.channels_snapshot = Some(Arc::clone(&snapshot));
        snapshot
    }

    /// Deprecated alias for [`Self::channels`].
    #[deprecated(note = "use channels() which now returns Arc<[Channel]>")]
    pub fn channels_snapshot(&mut self) -> Arc<[Channel]> {
        self.channels()
    }

    fn build_channels(&self) -> Vec<Channel> {
        self.server_state.channels.values().cloned().collect()
    }

    /// Get all users from tsclientlib state as a shared snapshot.
    ///
    /// The snapshot is rebuilt after an event pump or a state mutation, so
    /// repeated UI reads in one frame share a single allocation.
    pub fn users(&mut self) -> Arc<[User]> {
        if let Some(snapshot) = &self.users_snapshot {
            return Arc::clone(snapshot);
        }
        let snapshot: Arc<[User]> = self.build_users().into();
        self.users_snapshot = Some(Arc::clone(&snapshot));
        snapshot
    }

    /// Deprecated alias for [`Self::users`].
    #[deprecated(note = "use users() which now returns Arc<[User]>")]
    pub fn users_snapshot(&mut self) -> Arc<[User]> {
        self.users()
    }

    fn build_users(&self) -> Vec<User> {
        let mut users: Vec<User> = self.server_state.users.values().cloned().collect();
        for user in &mut users {
            user.is_talking = self.talking_users.contains_key(&user.id);
        }
        // Apply local mute override for self — tsclientlib's state book
        // doesn't reflect our clientupdate, so any re-read from it will
        // have stale input_muted. We always apply our known state.
        if let (Some(client_id), Some(muted)) = (self.client_id, self.self_input_muted) {
            if let Some(user) = users.iter_mut().find(|u| u.id == client_id) {
                user.is_input_muted = muted;
            }
        }
        users
    }

    /// Drop cached snapshots after a state mutation.
    fn invalidate_snapshots(&mut self) {
        self.users_snapshot = None;
        self.channels_snapshot = None;
    }

    /// Get all users directly from tsclientlib's connection state.
    ///
    /// This bypasses `server_state` and reads directly from the live
    /// connection book, which may have users that `server_state` missed.
    pub fn users_from_connection(&self) -> Vec<User> {
        let con = match self.connection.as_ref() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let state = match con.get_state() {
            Ok(s) => s,
            Err(e) => {
                warn!("users_from_connection: get_state failed: {}", e);
                return Vec::new();
            }
        };
        debug!(
            "users_from_connection: tsclientlib state has {} clients, {} channels",
            state.clients.len(),
            state.channels.len()
        );
        let mut users: Vec<User> = state.clients.values().map(|c| ts_client_to_user(c)).collect();
        for user in &mut users {
            user.is_talking = self.talking_users.contains_key(&user.id);
        }
        // Apply local mute override (same as users())
        if let (Some(client_id), Some(muted)) = (self.client_id, self.self_input_muted) {
            if let Some(user) = users.iter_mut().find(|u| u.id == client_id) {
                user.is_input_muted = muted;
            }
        }
        users
    }

    /// Get a specific channel
    pub fn channel(&self, id: u64) -> Option<Channel> {
        self.server_state.channels.get(&id).cloned()
    }

    /// Get a specific user
    pub fn user(&self, id: u16) -> Option<User> {
        self.server_state.users.get(&id).cloned()
    }

    /// Get current connection quality metrics (RTT, packet loss, bandwidth).
    ///
    /// Returns `None` while not connected to a server.
    pub fn get_network_stats(&self) -> Option<NetworkStats> {
        let con = self.connection.as_ref()?;
        let stats = con.get_network_stats().ok()?;
        // PacketStat order: InControl, InKeepalive, InSpeech,
        // OutControl, OutKeepalive, OutSpeech
        let last_second = stats.get_last_second_bytes();
        let received: u32 = last_second[PacketStat::InControl as usize]
            + last_second[PacketStat::InKeepalive as usize]
            + last_second[PacketStat::InSpeech as usize];
        let sent: u32 = last_second[PacketStat::OutControl as usize]
            + last_second[PacketStat::OutKeepalive as usize]
            + last_second[PacketStat::OutSpeech as usize];
        // Same packet classes as get_packetloss()'s denominator:
        // voice-in + ping/pong-in + ack-in + ack/command-out
        let loss_observed = stats.total_packets[PacketStat::InSpeech as usize]
            + stats.total_packets[PacketStat::InKeepalive as usize]
            + stats.total_packets[PacketStat::InControl as usize]
            + stats.total_packets[PacketStat::OutControl as usize];
        Some(NetworkStats {
            rtt_ms: stats.rtt.as_secs_f64() * 1000.0,
            rtt_dev_ms: stats.rtt_dev.as_secs_f64() * 1000.0,
            packet_loss: stats.get_packetloss(),
            packet_loss_in: stats.get_packetloss_s2c_total(),
            bytes_received_per_sec: received as u64,
            bytes_sent_per_sec: sent as u64,
            loss_observed_total: loss_observed,
        })
    }

    /// Move to a channel
    pub fn move_to_channel(&mut self, channel_id: u64) -> Result<()> {
        self.move_to_channel_with_password(channel_id, None)
    }

    /// Move to a channel with password
    pub fn move_to_channel_with_password(
        &mut self,
        channel_id: u64,
        password: Option<String>,
    ) -> Result<()> {
        self.ensure_connected()?;
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;

        // Get our client ID from the state
        let our_client_id = con
            .get_state()
            .map_err(|e| Error::Internal(e.to_string()))?
            .own_client;

        debug!("Moving client {} to channel {}", our_client_id.0, channel_id);

        // Create the clientmove command
        let mut cmd = OutCommand::new(
            Direction::C2S,
            Flags::empty(),
            PacketType::Command,
            "clientmove",
        );
        cmd.write_arg("clid", &our_client_id.0);
        cmd.write_arg("cid", &channel_id);

        // Add password if provided
        if let Some(pwd) = password {
            let encoded_pwd = tsproto_types::crypto::encode_password(pwd.as_bytes());
            cmd.write_arg("cpw", &encoded_pwd);
        }

        // Send the command using OutCommandExt trait
        cmd.send(con)
            .map_err(|e| Error::Internal(e.to_string()))?;

        // `channel_id` is updated from the server's book event
        // (`PropertyId::ClientChannel`) once the move is confirmed. Updating it
        // here would leave a stale value if the server rejects the move.
        Ok(())
    }

    /// Send a message to the server
    pub fn send_server_message(&mut self, message: impl Into<String>) -> Result<()> {
        self.ensure_connected()?;
        let msg = message.into();
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;

        debug!("Sending server message ({} chars)", msg.chars().count());

        // Get state and create message command, then send it
        con.get_state()
            .map_err(|e| Error::Internal(e.to_string()))?
            .send_message(TsMessageTarget::Server, &msg)
            .send(con)
            .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(())
    }

    /// Send a message to the current channel
    pub fn send_channel_message(&mut self, message: impl Into<String>) -> Result<()> {
        self.ensure_connected()?;
        let msg = message.into();
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;

        debug!("Sending channel message ({} chars)", msg.chars().count());

        // Get state and create message command, then send it
        con.get_state()
            .map_err(|e| Error::Internal(e.to_string()))?
            .send_message(TsMessageTarget::Channel, &msg)
            .send(con)
            .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(())
    }

    /// Send a private message to a user
    pub fn send_private_message(&mut self, user_id: u16, message: impl Into<String>) -> Result<()> {
        self.ensure_connected()?;
        let msg = message.into();
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;

        debug!(
            "Sending private message to {} ({} chars)",
            user_id,
            msg.chars().count()
        );

        // Get state and create message command, then send it
        con.get_state()
            .map_err(|e| Error::Internal(e.to_string()))?
            .send_message(TsMessageTarget::Client(ClientId(user_id)), &msg)
            .send(con)
            .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(())
    }

    /// Send a poke to a user
    pub fn send_poke(&mut self, user_id: u16, message: impl Into<String>) -> Result<()> {
        self.ensure_connected()?;
        let msg = message.into();
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;

        debug!(
            "Sending poke to {} ({} chars)",
            user_id,
            msg.chars().count()
        );

        con.get_state()
            .map_err(|e| Error::Internal(e.to_string()))?
            .send_message(TsMessageTarget::Poke(ClientId(user_id)), &msg)
            .send(con)
            .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(())
    }

    /// Send audio data
    pub fn send_audio(&mut self, data: &[u8], codec: AudioCodec) -> Result<()> {
        validate_audio_payload(data.len())?;
        validate_whisper_targets(&self.whisper_clients, &self.whisper_channels)?;
        self.ensure_connected()?;

        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;

        if !con.can_send_audio() {
            return Err(Error::Internal("Cannot send audio".to_string()));
        }

        // Whisper mode: route to the explicit target set instead of the channel
        let audio_data = if !self.whisper_clients.is_empty() || !self.whisper_channels.is_empty() {
            AudioData::C2SWhisper {
                id: self.audio_sequence,
                codec: audio_codec_to_codec_type(codec),
                channels: self.whisper_channels.clone(),
                clients: self.whisper_clients.clone(),
                data,
            }
        } else {
            AudioData::C2S {
                id: self.audio_sequence,
                codec: audio_codec_to_codec_type(codec),
                data,
            }
        };
        self.audio_sequence = self.audio_sequence.wrapping_add(1);

        let packet = OutAudio::new(&audio_data);
        con.send_audio(packet)
            .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(())
    }

    /// Set the whisper target set. Voice captured afterwards is sent as
    /// `C2SWhisper` packets to these clients/channels instead of the current
    /// channel; passing two empty lists switches back to channel talk.
    /// Targets are per-packet client/channel ids — resolve them from the
    /// current user list at call time, they don't follow reconnects.
    pub fn set_whisper_targets(&mut self, clients: Vec<u16>, channels: Vec<u64>) -> Result<()> {
        validate_whisper_targets(&clients, &channels)?;
        self.whisper_clients = clients;
        self.whisper_channels = channels;
        if self.whisper_clients.is_empty() && self.whisper_channels.is_empty() {
            debug!("Whisper mode off, back to channel talk");
        } else {
            debug!(
                "Whisper mode on: clients={:?}, channels={:?}",
                self.whisper_clients, self.whisper_channels
            );
        }
        Ok(())
    }

    /// Download a file from the server
    pub fn download_file(&mut self, channel_id: u64, path: &str) -> Result<()> {
        self.download_file_with_password(channel_id, path, None)
    }

    /// Download a file from a channel, optionally protected by a password.
    pub fn download_file_with_password(
        &mut self,
        channel_id: u64,
        path: &str,
        password: Option<&str>,
    ) -> Result<()> {
        self.start_download(channel_id, path, password, None, 0)
    }

    /// Download a file directly to a path, without buffering it in memory.
    ///
    /// `offset` resumes a partial download: the file is opened in append mode
    /// and the server is asked to seek to that position.
    pub fn download_file_to_path(
        &mut self,
        channel_id: u64,
        path: &str,
        destination: impl AsRef<Path>,
        offset: u64,
    ) -> Result<()> {
        self.download_file_to_path_with_password(channel_id, path, destination, None, offset)
    }

    /// Password-protected path download with resume support.
    pub fn download_file_to_path_with_password(
        &mut self,
        channel_id: u64,
        path: &str,
        destination: impl AsRef<Path>,
        password: Option<&str>,
        offset: u64,
    ) -> Result<()> {
        self.start_download(
            channel_id,
            path,
            password,
            Some(destination.as_ref().to_path_buf()),
            offset,
        )
    }

    fn start_download(
        &mut self,
        channel_id: u64,
        path: &str,
        password: Option<&str>,
        destination: Option<PathBuf>,
        offset: u64,
    ) -> Result<()> {
        self.ensure_connected()?;
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;

        let seek = if offset == 0 { None } else { Some(offset) };
        let handle = con
            .download_file(ChannelId(channel_id), path, password, seek)
            .map_err(|e| Error::Internal(e.to_string()))?;

        self.pending_downloads.insert(
            handle.0,
            PendingDownload {
                channel_id,
                path: path.to_string(),
                destination,
                offset,
            },
        );

        debug!(
            "Started download: channel={}, path={}, offset={}",
            channel_id, path, offset
        );
        Ok(())
    }

    /// Upload a file to the server
    pub fn upload_file(&mut self, channel_id: u64, path: &str, data: &[u8], overwrite: bool) -> Result<()> {
        self.upload_file_with_password(channel_id, path, data, overwrite, None)
    }

    /// Upload a file to a channel, optionally protected by a password.
    pub fn upload_file_with_password(
        &mut self,
        channel_id: u64,
        path: &str,
        data: &[u8],
        overwrite: bool,
        password: Option<&str>,
    ) -> Result<()> {
        self.ensure_connected()?;
        let con = self.connection.as_mut().ok_or(ConnectionError::NotConnected)?;
        let handle = con.upload_file(
            ChannelId(channel_id), path, password, data.len() as u64, overwrite, false,
        ).map_err(|e| Error::Internal(e.to_string()))?;
        self.pending_uploads.insert(handle.0, PendingUpload {
            channel_id,
            path: path.to_string(),
            size: data.len() as u64,
            source: UploadSource::Memory(data.to_vec()),
            password: password.map(str::to_string),
        });
        debug!("Started upload: channel={}, path={}, size={}", channel_id, path, data.len());
        Ok(())
    }

    /// Upload a file from disk without reading it into memory.
    pub fn upload_file_from_path(
        &mut self,
        channel_id: u64,
        path: &str,
        source: impl AsRef<Path>,
        overwrite: bool,
    ) -> Result<()> {
        self.upload_file_from_path_with_password(
            channel_id,
            path,
            source,
            overwrite,
            None,
            false,
        )
    }

    /// Password-protected path upload with optional server-side resume.
    pub fn upload_file_from_path_with_password(
        &mut self,
        channel_id: u64,
        path: &str,
        source: impl AsRef<Path>,
        overwrite: bool,
        password: Option<&str>,
        resume: bool,
    ) -> Result<()> {
        let source = source.as_ref().to_path_buf();
        let size = std::fs::metadata(&source)
            .map_err(|error| Error::InvalidArgument(format!("cannot stat upload source: {error}")))?
            .len();

        self.ensure_connected()?;
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;
        let handle = con
            .upload_file(
                ChannelId(channel_id),
                path,
                password,
                size,
                overwrite,
                resume,
            )
            .map_err(|e| Error::Internal(e.to_string()))?;

        self.pending_uploads.insert(
            handle.0,
            PendingUpload {
                channel_id,
                path: path.to_string(),
                size,
                source: UploadSource::File(source),
                password: password.map(str::to_string),
            },
        );
        debug!(
            "Started path upload: channel={}, path={}, size={}, resume={}",
            channel_id, path, size, resume
        );
        Ok(())
    }

    /// Ask the server for the size it persisted after an upload stream closed.
    ///
    /// `FileUploaded` is deferred until the `ftgetfileinfo` response confirms
    /// the size. If the request cannot be sent (for example the connection
    /// dropped after the transfer), the upload is reported as successful with
    /// a warning instead of hanging in a pending state.
    fn verify_uploaded_file(
        &mut self,
        channel_id: u64,
        path: &str,
        expected_size: u64,
        password: Option<&str>,
    ) -> Vec<Event> {
        let send_result = match self.connection.as_mut() {
            Some(con) => {
                let mut cmd = OutCommand::new(
                    Direction::C2S,
                    Flags::empty(),
                    PacketType::Command,
                    "ftgetfileinfo",
                );
                cmd.write_arg("cid", &channel_id);
                cmd.write_arg("cpw", &encode_channel_password(password));
                cmd.write_arg("name", &path);
                cmd.send_with_result(con).map_err(|e| e.to_string())
            }
            None => Err("not connected".to_string()),
        };

        match send_result {
            Ok(handle) => {
                self.pending_file_infos.insert(
                    handle.0,
                    PendingFileInfo {
                        channel_id,
                        path: path.to_string(),
                        expected_size,
                    },
                );
                debug!(
                    "Requested upload verification: channel={}, path={}, size={}",
                    channel_id, path, expected_size
                );
                Vec::new()
            }
            Err(error) => {
                warn!(
                    "Upload verification request failed for {}: {}; reporting success",
                    path, error
                );
                let event = Event::FileUploaded {
                    channel_id,
                    path: path.to_string(),
                };
                let _ = self.event_tx.send(event.clone());
                vec![event]
            }
        }
    }

    /// Request metadata for one file on the server.
    ///
    /// The result arrives as `Event::FileInfoReceived`.
    pub fn get_file_info(&mut self, channel_id: u64, path: &str) -> Result<()> {
        self.ensure_connected()?;
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;
        let mut cmd = OutCommand::new(
            Direction::C2S,
            Flags::empty(),
            PacketType::Command,
            "ftgetfileinfo",
        );
        cmd.write_arg("cid", &channel_id);
        cmd.write_arg("cpw", &"");
        cmd.write_arg("name", &path);
        let handle = cmd
            .send_with_result(con)
            .map_err(|e| Error::Internal(e.to_string()))?;
        self.pending_file_info_queries.insert(
            handle.0,
            PendingFileInfoQuery {
                channel_id,
                path: path.to_string(),
            },
        );
        debug!("Requested file info: channel={}, path={}", channel_id, path);
        Ok(())
    }

    /// List active file transfers (`ftlist`).
    ///
    /// The result arrives as `Event::FileTransfersListed`.
    pub fn list_file_transfers(&mut self) -> Result<()> {
        self.ensure_connected()?;
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;
        let cmd = OutCommand::new(
            Direction::C2S,
            Flags::empty(),
            PacketType::Command,
            "ftlist",
        );
        cmd.send(con)
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }

    /// Stop a file transfer (`ftstop`).
    ///
    /// The result arrives as `Event::FileTransferStopped`.
    pub fn stop_file_transfer(
        &mut self,
        server_filetransfer_id: u16,
        client_filetransfer_id: u16,
        delete: bool,
    ) -> Result<()> {
        self.ensure_connected()?;
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;
        let mut cmd = OutCommand::new(
            Direction::C2S,
            Flags::empty(),
            PacketType::Command,
            "ftstop",
        );
        cmd.write_arg("serverftfid", &server_filetransfer_id);
        cmd.write_arg("clientftfid", &client_filetransfer_id);
        cmd.write_arg("delete", &(delete as u8));
        cmd.send(con)
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }

    /// Request the file list for a channel directory
    pub fn list_files(&mut self, channel_id: u64, path: &str) -> Result<()> {
        self.list_files_with_password(channel_id, path, None)
    }

    /// Request the file list for a password-protected channel directory.
    pub fn list_files_with_password(
        &mut self,
        channel_id: u64,
        path: &str,
        password: Option<&str>,
    ) -> Result<()> {
        if self
            .pending_file_lists
            .values()
            .any(|pending| pending.channel_id == channel_id && pending.path == path)
        {
            return Err(Error::InvalidArgument(
                "file list request already in flight".to_string(),
            ));
        }
        self.ensure_connected()?;
        let con = self.connection.as_mut().ok_or(ConnectionError::NotConnected)?;
        let mut cmd = OutCommand::new(Direction::C2S, Flags::empty(), PacketType::Command, "ftgetfilelist");
        cmd.write_arg("cid", &channel_id);
        cmd.write_arg("cpw", &encode_channel_password(password));
        cmd.write_arg("path", &path);
        let handle = cmd
            .send_with_result(con)
            .map_err(|e| Error::Internal(e.to_string()))?;
        self.pending_file_lists.insert(
            handle.0,
            PendingFileList {
                channel_id,
                path: path.to_string(),
                entries: Vec::new(),
            },
        );
        debug!("Requested file list: channel={}, path={}", channel_id, path);
        Ok(())
    }

    /// Query effective permissions for the current user in a channel.
    /// Sends `permoverview cldbid=X cid=Y` and collects responses asynchronously.
    /// Results arrive as ChannelPermissionsUpdated events.
    pub fn query_channel_permissions(&mut self, channel_id: u64) -> Result<()> {
        if self
            .pending_perm_overviews
            .values()
            .any(|pending| pending.channel_id == channel_id)
        {
            return Err(Error::InvalidArgument(
                "permission request already in flight".to_string(),
            ));
        }
        self.ensure_connected()?;
        let con = self.connection.as_mut().ok_or(ConnectionError::NotConnected)?;
        let state = con.get_state().map_err(|e| Error::Internal(e.to_string()))?;
        let our_client = state.clients.get(&state.own_client)
            .ok_or(Error::Internal("Own client not found".to_string()))?;
        let cldbid = our_client.database_id.0;
        debug!("query_channel_permissions: cldbid={}, cid={}", cldbid, channel_id);
        let mut cmd = OutCommand::new(Direction::C2S, Flags::empty(), PacketType::Command, "permoverview");
        cmd.write_arg("cldbid", &cldbid);
        cmd.write_arg("cid", &channel_id);
        cmd.write_arg("permid", &0u32);
        let handle = cmd
            .send_with_result(con)
            .map_err(|e| Error::Internal(e.to_string()))?;
        self.pending_perm_overviews.insert(
            handle.0,
            PendingPermOverview {
                channel_id,
                perms: HashMap::new(),
            },
        );
        Ok(())
    }

    /// Delete a file on the server
    pub fn delete_file(&mut self, channel_id: u64, name: &str) -> Result<()> {
        self.delete_file_with_password(channel_id, name, None)
    }

    /// Delete a file in a password-protected channel.
    pub fn delete_file_with_password(
        &mut self,
        channel_id: u64,
        name: &str,
        password: Option<&str>,
    ) -> Result<()> {
        self.ensure_connected()?;
        let con = self.connection.as_mut().ok_or(ConnectionError::NotConnected)?;
        let mut cmd = OutCommand::new(Direction::C2S, Flags::empty(), PacketType::Command, "ftdeletefile");
        cmd.write_arg("cid", &channel_id);
        cmd.write_arg("cpw", &encode_channel_password(password));
        cmd.write_arg("name", &name);
        cmd.send(con).map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }

    /// Rename a file on the server
    pub fn rename_file(&mut self, channel_id: u64, old_name: &str, new_name: &str) -> Result<()> {
        self.rename_file_with_password(channel_id, old_name, new_name, None)
    }

    /// Rename a file in a password-protected channel.
    pub fn rename_file_with_password(
        &mut self,
        channel_id: u64,
        old_name: &str,
        new_name: &str,
        password: Option<&str>,
    ) -> Result<()> {
        self.ensure_connected()?;
        let con = self.connection.as_mut().ok_or(ConnectionError::NotConnected)?;
        let encoded = encode_channel_password(password);
        let mut cmd = OutCommand::new(Direction::C2S, Flags::empty(), PacketType::Command, "ftrenamefile");
        cmd.write_arg("cid", &channel_id);
        cmd.write_arg("cpw", &encoded);
        cmd.write_arg("tcid", &channel_id);
        cmd.write_arg("tcpw", &encoded);
        cmd.write_arg("oldname", &old_name);
        cmd.write_arg("newname", &new_name);
        cmd.send(con).map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }

    /// Create a directory on the server
    pub fn create_directory(&mut self, channel_id: u64, dirname: &str) -> Result<()> {
        self.create_directory_with_password(channel_id, dirname, None)
    }

    /// Create a directory in a password-protected channel.
    pub fn create_directory_with_password(
        &mut self,
        channel_id: u64,
        dirname: &str,
        password: Option<&str>,
    ) -> Result<()> {
        self.ensure_connected()?;
        let con = self.connection.as_mut().ok_or(ConnectionError::NotConnected)?;
        let mut cmd = OutCommand::new(Direction::C2S, Flags::empty(), PacketType::Command, "ftcreatedir");
        cmd.write_arg("cid", &channel_id);
        cmd.write_arg("cpw", &encode_channel_password(password));
        cmd.write_arg("dirname", &dirname);
        cmd.send(con).map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }

    /// Subscribe to events
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.event_tx.subscribe()
    }

    /// Disconnect from the server
    pub fn disconnect(&mut self) -> Result<()> {
        self.disconnect_with_reason("Goodbye")
    }

    /// Disconnect from the server with a reason
    pub fn disconnect_with_reason(&mut self, reason: impl Into<String>) -> Result<()> {
        info!("Disconnecting from server");

        if let Some(con) = self.connection.as_mut() {
            let options = DisconnectOptions::new()
                .reason(Reason::Clientdisconnect)
                .message(reason.into());

            con.disconnect(options)
                .map_err(|e| ConnectionError::ConnectionLost(e.to_string()))?;
        }

        // Don't take/drop the connection here — keep it alive so the runtime
        // has time to send the disconnect packet over the network.
        // The connection will be dropped when the Client is dropped (via close/nativeDestroy).

        self.state = ConnectionState::Disconnected;

        let _ = self.event_tx.send(Event::Disconnected {
            reason: "User requested".to_string(),
        });

        Ok(())
    }

    /// Get access to the underlying tsclientlib connection
    pub fn inner(&self) -> Option<&TsConnection> {
        self.connection.as_ref()
    }

    /// Get mutable access to the underlying tsclientlib connection
    pub fn inner_mut(&mut self) -> Option<&mut TsConnection> {
        self.connection.as_mut()
    }

    /// Notify the server of our input muted state
    pub fn set_input_muted(&mut self, muted: bool) -> Result<()> {
        self.ensure_connected()?;
        let con = self
            .connection
            .as_mut()
            .ok_or(ConnectionError::NotConnected)?;

        let val: u32 = if muted { 1 } else { 0 };
        let mut cmd = OutCommand::new(
            Direction::C2S,
            Flags::empty(),
            PacketType::Command,
            "clientupdate",
        );
        cmd.write_arg("client_input_muted", &val);

        cmd.send(con)
            .map_err(|e| Error::Internal(e.to_string()))?;

        info!("Sent clientupdate client_input_muted={}", val);

        // Track locally — tsclientlib's state book won't reflect this
        // because the server doesn't echo clientupdate back to the sender.
        self.self_input_muted = Some(muted);
        self.invalidate_snapshots();

        // Also patch server_state immediately
        if let Some(client_id) = self.client_id {
            if let Some(user) = self.server_state.users.get_mut(&client_id) {
                user.is_input_muted = muted;
            }
        }

        Ok(())
    }
}

/// Convert tsclientlib CodecType to our AudioCodec
fn codec_type_to_audio_codec(codec: CodecType) -> AudioCodec {
    match codec {
        CodecType::SpeexNarrowband => AudioCodec::SpeexNarrowband,
        CodecType::SpeexWideband => AudioCodec::SpeexWideband,
        CodecType::SpeexUltrawideband => AudioCodec::SpeexUltraWideband,
        CodecType::CeltMono => AudioCodec::CeltMono,
        CodecType::OpusVoice => AudioCodec::OpusVoice,
        CodecType::OpusMusic => AudioCodec::OpusMusic,
    }
}

/// Convert our AudioCodec to tsclientlib CodecType
fn audio_codec_to_codec_type(codec: AudioCodec) -> CodecType {
    match codec {
        AudioCodec::SpeexNarrowband => CodecType::SpeexNarrowband,
        AudioCodec::SpeexWideband => CodecType::SpeexWideband,
        AudioCodec::SpeexUltraWideband => CodecType::SpeexUltrawideband,
        AudioCodec::CeltMono => CodecType::CeltMono,
        AudioCodec::OpusVoice => CodecType::OpusVoice,
        AudioCodec::OpusMusic => CodecType::OpusMusic,
    }
}

/// Convert an event invoker into a user snapshot, preferring the live state.
fn invoker_to_user(
    invoker: Option<&tsproto_types::Invoker>,
    state: &ServerState,
) -> Option<User> {
    let invoker = invoker?;
    if let Some(user) = state.users.get(&invoker.id.0) {
        return Some(user.clone());
    }
    let uid = invoker
        .uid
        .as_ref()
        .map(|uid| base64::engine::general_purpose::STANDARD.encode(&uid.0))
        .unwrap_or_default();
    Some(User {
        id: invoker.id.0,
        uid,
        nickname: invoker.name.clone(),
        ..Default::default()
    })
}

/// Convert tsclientlib Client to our User type
fn ts_client_to_user(client: &TsClient) -> User {
    // Convert UID bytes to base64 string
    let uid = client.uid.as_ref()
        .map(|u| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &u.0))
        .unwrap_or_default();

    // Check client type
    let client_type = if matches!(client.client_type, TsClientType::Query { .. }) { 1 } else { 0 };

    User {
        id: client.id.0,
        uid,
        database_id: client.database_id.0,
        channel_id: client.channel.0,
        nickname: client.name.clone(),
        client_type,
        is_talking: false, // Tracked separately via audio
        is_input_muted: client.input_muted,
        is_output_muted: client.output_muted,
        has_input_hardware: client.input_hardware_enabled,
        has_output_hardware: client.output_hardware_enabled,
        is_away: client.away_message.is_some(),
        away_message: client.away_message.clone(),
        is_recording: client.is_recording,
        is_priority_speaker: client.is_priority_speaker,
        is_channel_commander: client.is_channel_commander,
        talk_power: client.talk_power,
        is_talker: client.talk_power_granted,
        server_groups: client.server_groups.iter().map(|g| g.0).collect(),
        channel_group: client.channel_group.0,
        platform: client.optional_data.as_ref().map(|o| o.platform.clone()).unwrap_or_default(),
        version: client.optional_data.as_ref().map(|o| o.version.clone()).unwrap_or_default(),
        country: if client.country_code.is_empty() {
            None
        } else {
            Some(client.country_code.clone())
        },
        description: if client.description.is_empty() {
            None
        } else {
            Some(client.description.clone())
        },
        avatar_id: if client.avatar_hash.is_empty() { None } else { Some(client.avatar_hash.clone()) },
        icon_id: client.icon.0 as i64,
        idle_time: client
            .connection_data
            .as_ref()
            .map(|data| data.idle_time.whole_seconds().max(0) as u64)
            .unwrap_or(0),
        connected_time: client
            .connection_data
            .as_ref()
            .and_then(|data| data.connected_time)
            .map(|duration| duration.whole_seconds().max(0) as u64)
            .unwrap_or(0),
        created_at: client
            .optional_data
            .as_ref()
            .map(|data| data.created.unix_timestamp()),
        last_connected_at: client
            .optional_data
            .as_ref()
            .map(|data| data.last_connected.unix_timestamp()),
    }
}

/// Convert a dependency `MaxClients` value into the legacy `i32` plus the
/// structured kind that preserves Unlimited/Inherited/Unknown.
fn max_clients_from_ts(value: Option<tsproto_types::MaxClients>) -> (i32, MaxClientsKind) {
    match value {
        Some(tsproto_types::MaxClients::Unlimited) => (-1, MaxClientsKind::Unlimited),
        Some(tsproto_types::MaxClients::Inherited) => (-1, MaxClientsKind::Inherited),
        Some(tsproto_types::MaxClients::Limited(n)) => (n as i32, MaxClientsKind::Limited(n)),
        None => (-1, MaxClientsKind::Unknown),
    }
}

/// Convert tsclientlib Channel to our Channel type
fn ts_channel_to_channel(channel: &TsChannel) -> Channel {
    let (is_permanent, is_semi_permanent) = match channel.channel_type {
        TsChannelType::Permanent => (true, false),
        TsChannelType::SemiPermanent => (false, true),
        TsChannelType::Temporary => (false, false),
    };

    // Extract description from optional data
    let description = channel.optional_data.as_ref().map(|o| o.description.clone());

    let (max_clients, max_clients_kind) = max_clients_from_ts(channel.max_clients);
    let (max_family_clients, max_family_clients_kind) =
        max_clients_from_ts(channel.max_family_clients);

    Channel {
        id: channel.id.0,
        parent_id: channel.parent.0,
        name: channel.name.clone(),
        topic: channel.topic.clone(),
        description,
        order: channel.order.0,
        is_permanent,
        is_semi_permanent,
        is_default: channel.is_default.unwrap_or(false),
        has_password: channel.has_password.unwrap_or(false),
        codec: ts_codec_to_u8(channel.codec),
        codec_quality: channel.codec_quality.unwrap_or(7),
        max_clients,
        max_clients_kind,
        max_family_clients,
        max_family_clients_kind,
        needed_talk_power: channel.needed_talk_power.unwrap_or(0),
        icon_id: channel.icon.map(|i| i.0 as i64).unwrap_or(0),
        is_subscribed: channel.subscribed,
        permission_hints: {
            let hints = channel.permission_hints.map(|h| h.bits() as u64).unwrap_or(0);
            if hints != 0 {
                debug!("Channel {} ({}) permission_hints = {:#x}", channel.name, channel.id.0, hints);
            }
            hints
        },
        permission_hints_known: channel.permission_hints.is_some(),
    }
}

/// Convert tsclientlib Codec to u8
fn ts_codec_to_u8(codec: TsCodec) -> u8 {
    match codec {
        TsCodec::SpeexNarrowband => 0,
        TsCodec::SpeexWideband => 1,
        TsCodec::SpeexUltrawideband => 2,
        TsCodec::CeltMono => 3,
        TsCodec::OpusVoice => 4,
        TsCodec::OpusMusic => 5,
    }
}

/// Permission IDs for file operations (from Permissions.csv, 0-indexed from line 2)
const PERM_FT_FILE_UPLOAD_POWER: u32 = 235;
const PERM_FT_NEEDED_FILE_UPLOAD_POWER: u32 = 236;
const PERM_FT_FILE_DOWNLOAD_POWER: u32 = 237;
const PERM_FT_NEEDED_FILE_DOWNLOAD_POWER: u32 = 238;
const PERM_FT_FILE_DELETE_POWER: u32 = 239;
const PERM_FT_NEEDED_FILE_DELETE_POWER: u32 = 240;
const PERM_FT_FILE_RENAME_POWER: u32 = 241;
const PERM_FT_NEEDED_FILE_RENAME_POWER: u32 = 242;
const PERM_FT_FILE_BROWSE_POWER: u32 = 243;
const PERM_FT_NEEDED_FILE_BROWSE_POWER: u32 = 244;
const PERM_FT_DIRECTORY_CREATE_POWER: u32 = 245;
const PERM_FT_NEEDED_DIRECTORY_CREATE_POWER: u32 = 246;

/// ChannelPermissionHint bitflags (matches Enums.toml / Channel.java constants)
const HINT_JOIN: u64 = 1;
const HINT_FILE_UPLOAD: u64 = 64;
const HINT_FILE_DOWNLOAD: u64 = 128;
const HINT_FILE_DELETE: u64 = 256;
const HINT_FILE_RENAME: u64 = 512;
const HINT_FILE_BROWSE: u64 = 1024;
const HINT_FILE_DIRECTORY_CREATE: u64 = 2048;

/// Compute ChannelPermissionHint bitflags from a permoverview result.
/// For each file operation, the user can do it if their power >= the needed power.
/// Permissions not present in the map default to 0.
///
/// The returned hints are marked `known`: this is only called for a response
/// that arrived, while a failed query uses [`PermissionHints::UNKNOWN`].
fn compute_file_permission_hints(perms: &HashMap<u32, i32>) -> PermissionHints {
    let mut hints: u64 = HINT_JOIN; // If we're in the channel, we can join

    let can = |power_id: u32, needed_id: u32| -> bool {
        match perms.get(&power_id) {
            Some(&-1) => true, // unlimited (admin)
            Some(&power) => power >= perms.get(&needed_id).copied().unwrap_or(0),
            // Missing from permoverview means the server did not grant it;
            // do not show a file action the user probably cannot perform.
            None => false,
        }
    };

    if can(PERM_FT_FILE_UPLOAD_POWER, PERM_FT_NEEDED_FILE_UPLOAD_POWER) {
        hints |= HINT_FILE_UPLOAD;
    }
    if can(PERM_FT_FILE_DOWNLOAD_POWER, PERM_FT_NEEDED_FILE_DOWNLOAD_POWER) {
        hints |= HINT_FILE_DOWNLOAD;
    }
    if can(PERM_FT_FILE_DELETE_POWER, PERM_FT_NEEDED_FILE_DELETE_POWER) {
        hints |= HINT_FILE_DELETE;
    }
    if can(PERM_FT_FILE_RENAME_POWER, PERM_FT_NEEDED_FILE_RENAME_POWER) {
        hints |= HINT_FILE_RENAME;
    }
    if can(PERM_FT_FILE_BROWSE_POWER, PERM_FT_NEEDED_FILE_BROWSE_POWER) {
        hints |= HINT_FILE_BROWSE;
    }
    if can(PERM_FT_DIRECTORY_CREATE_POWER, PERM_FT_NEEDED_DIRECTORY_CREATE_POWER) {
        hints |= HINT_FILE_DIRECTORY_CREATE;
    }

    debug!(
        "compute_file_permission_hints: {} perms -> hints={:#x}",
        perms.len(),
        hints
    );
    PermissionHints::known(hints)
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn accepts_audio_payload_at_limit() {
        assert!(validate_audio_payload(MAX_AUDIO_PAYLOAD).is_ok());
    }

    #[test]
    fn rejects_oversized_audio_payload() {
        let err = validate_audio_payload(MAX_AUDIO_PAYLOAD + 1).unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)));
    }

    #[test]
    fn accepts_whisper_targets_at_limits() {
        let clients = vec![1; MAX_WHISPER_CLIENTS];
        let channels = vec![1; MAX_WHISPER_CHANNELS];
        assert!(validate_whisper_targets(&clients, &channels).is_ok());
    }

    #[test]
    fn rejects_too_many_whisper_clients() {
        let clients = vec![1; MAX_WHISPER_CLIENTS + 1];
        let err = validate_whisper_targets(&clients, &[]).unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)));
    }

    #[test]
    fn rejects_too_many_whisper_channels() {
        let channels = vec![1; MAX_WHISPER_CHANNELS + 1];
        let err = validate_whisper_targets(&[], &channels).unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)));
    }

    #[test]
    fn allows_empty_whisper_targets_to_disable_whisper() {
        assert!(validate_whisper_targets(&[], &[]).is_ok());
    }

    #[test]
    fn channel_password_is_encoded() {
        assert_eq!(encode_channel_password(None), "");
        let expected = tsproto_types::crypto::encode_password(b"secret");
        assert_eq!(encode_channel_password(Some("secret")), expected);
        assert_ne!(encode_channel_password(Some("secret")), "secret");
    }

    #[test]
    fn duplicate_file_list_request_is_rejected() {
        let mut client = disconnected_client();
        client.pending_file_lists.insert(
            1,
            PendingFileList {
                channel_id: 5,
                path: "/".to_string(),
                entries: Vec::new(),
            },
        );
        assert!(matches!(
            client.list_files(5, "/"),
            Err(Error::InvalidArgument(_))
        ));
    }

    #[test]
    fn duplicate_permission_request_is_rejected() {
        let mut client = disconnected_client();
        client.pending_perm_overviews.insert(
            1,
            PendingPermOverview {
                channel_id: 5,
                perms: HashMap::new(),
            },
        );
        assert!(matches!(
            client.query_channel_permissions(5),
            Err(Error::InvalidArgument(_))
        ));
    }

    #[test]
    fn invoker_falls_back_to_a_minimal_user() {
        let invoker = tsproto_types::Invoker {
            name: "Kicker".to_string(),
            id: ClientId(7),
            uid: None,
        };
        let user = invoker_to_user(Some(&invoker), &ServerState::default()).unwrap();
        assert_eq!(user.id, 7);
        assert_eq!(user.nickname, "Kicker");
        assert!(user.uid.is_empty());
    }

    #[test]
    fn missing_file_permissions_are_not_allowed() {
        let hints = compute_file_permission_hints(&HashMap::new());
        let file_bits = HINT_FILE_UPLOAD
            | HINT_FILE_DOWNLOAD
            | HINT_FILE_DELETE
            | HINT_FILE_RENAME
            | HINT_FILE_BROWSE
            | HINT_FILE_DIRECTORY_CREATE;
        assert_eq!(hints.bits & file_bits, 0);
        assert!(hints.known);
    }

    #[test]
    fn unknown_permission_hints_are_marked_unknown() {
        assert_eq!(PermissionHints::UNKNOWN.bits, 0);
        assert!(!PermissionHints::UNKNOWN.known);
    }

    #[test]
    fn unlimited_power_allows_file_permissions() {
        let mut perms = HashMap::new();
        for id in [
            PERM_FT_FILE_UPLOAD_POWER,
            PERM_FT_NEEDED_FILE_UPLOAD_POWER,
            PERM_FT_FILE_DOWNLOAD_POWER,
            PERM_FT_NEEDED_FILE_DOWNLOAD_POWER,
            PERM_FT_FILE_DELETE_POWER,
            PERM_FT_NEEDED_FILE_DELETE_POWER,
            PERM_FT_FILE_RENAME_POWER,
            PERM_FT_NEEDED_FILE_RENAME_POWER,
            PERM_FT_FILE_BROWSE_POWER,
            PERM_FT_NEEDED_FILE_BROWSE_POWER,
            PERM_FT_DIRECTORY_CREATE_POWER,
            PERM_FT_NEEDED_DIRECTORY_CREATE_POWER,
        ] {
            perms.insert(id, -1);
        }
        let hints = compute_file_permission_hints(&perms);
        assert_ne!(hints.bits & HINT_FILE_UPLOAD, 0);
        assert_ne!(hints.bits & HINT_FILE_DOWNLOAD, 0);
        assert_ne!(hints.bits & HINT_FILE_DELETE, 0);
        assert_ne!(hints.bits & HINT_FILE_RENAME, 0);
        assert_ne!(hints.bits & HINT_FILE_BROWSE, 0);
        assert_ne!(hints.bits & HINT_FILE_DIRECTORY_CREATE, 0);
    }

    #[test]
    fn permission_power_must_meet_the_needed_power() {
        let mut perms = HashMap::new();
        perms.insert(PERM_FT_FILE_DOWNLOAD_POWER, 50);
        perms.insert(PERM_FT_NEEDED_FILE_DOWNLOAD_POWER, 10);
        assert_ne!(
            compute_file_permission_hints(&perms).bits & HINT_FILE_DOWNLOAD,
            0
        );

        perms.insert(PERM_FT_FILE_DOWNLOAD_POWER, 5);
        assert_eq!(
            compute_file_permission_hints(&perms).bits & HINT_FILE_DOWNLOAD,
            0
        );
    }

    #[test]
    fn users_report_talking_state() {
        let mut client = disconnected_client();
        client.server_state.users.insert(
            7,
            User {
                id: 7,
                nickname: "Talker".to_string(),
                ..Default::default()
            },
        );
        client.talking_users.insert(7, Instant::now());

        let users = client.users();
        assert_eq!(users.len(), 1);
        assert!(users[0].is_talking);
    }

    #[test]
    fn reconnect_watchdog_fires_once() {
        let mut client = disconnected_client();
        client.state = ConnectionState::Reconnecting;
        client.reconnect_started = Some(Instant::now() - Duration::from_secs(60));
        client.config.reconnect_watchdog = Some(Duration::from_secs(30));

        let event = client.reconnect_watchdog_event();
        assert!(matches!(
            event,
            Some(Event::ConnectionStalled { .. })
        ));
        assert!(client.reconnect_watchdog_event().is_none());
    }

    #[test]
    fn snapshots_are_shared_until_invalidated() {
        let mut client = disconnected_client();
        client.server_state.users.insert(
            1,
            User {
                id: 1,
                ..Default::default()
            },
        );
        client.server_state.channels.insert(
            10,
            Channel {
                id: 10,
                ..Default::default()
            },
        );

        let users_a = client.users();
        let users_b = client.users();
        assert!(Arc::ptr_eq(&users_a, &users_b));
        assert_eq!(users_a.len(), 1);

        let channels_a = client.channels();
        let channels_b = client.channels();
        assert!(Arc::ptr_eq(&channels_a, &channels_b));
        assert_eq!(channels_a.len(), 1);

        client.invalidate_snapshots();
        let users_c = client.users();
        assert!(!Arc::ptr_eq(&users_a, &users_c));
    }

    #[test]
    fn talk_timeout_only_expires_due_users() {
        let mut client = disconnected_client();
        for id in [3u16, 7] {
            client.server_state.users.insert(
                id,
                User {
                    id,
                    is_talking: true,
                    ..Default::default()
                },
            );
        }

        let now = Instant::now();
        let expired = now - Duration::from_millis(TALK_TIMEOUT_MS as u64 + 50);
        let live = now + Duration::from_secs(60);
        client.talking_users.insert(3, expired);
        client.talk_deadlines.insert((expired, 3));
        client.talking_users.insert(7, live);
        client.talk_deadlines.insert((live, 7));

        let events = client.check_talk_timeouts();

        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            Event::TalkStatusStop { user_id: 3 }
        ));
        assert!(!client.talking_users.contains_key(&3));
        assert!(!client.talk_deadlines.contains(&(expired, 3)));
        assert!(!client.server_state.users[&3].is_talking);
        assert!(client.talking_users.contains_key(&7));
        assert!(client.server_state.users[&7].is_talking);
    }

    #[test]
    fn stale_talk_deadline_is_ignored() {
        let mut client = disconnected_client();
        let now = Instant::now();
        let stale = now - Duration::from_secs(1);
        let live = now + Duration::from_secs(60);
        client.talking_users.insert(5, live);
        client.talk_deadlines.insert((stale, 5));

        let events = client.check_talk_timeouts();

        assert!(events.is_empty());
        assert_eq!(client.talking_users.get(&5), Some(&live));
        assert!(client.talk_deadlines.is_empty());
    }

    fn disconnected_client() -> Client {
        let config = ClientConfig::builder()
            .address("localhost:9987")
            .identity(crate::identity::Identity::create().unwrap())
            .nickname("test")
            .build()
            .unwrap();
        let (event_tx, _) = broadcast::channel(1);
        Client {
            connection: None,
            config,
            file_transfer: FileTransferConfig::default(),
            state: ConnectionState::Disconnected,
            server_state: ServerState::default(),
            event_tx,
            handlers: Vec::new(),
            client_id: None,
            channel_id: None,
            audio_sequence: 0,
            talking_users: HashMap::new(),
            talk_deadlines: BTreeSet::new(),
            pending_downloads: HashMap::new(),
            pending_uploads: HashMap::new(),
            pending_file_infos: HashMap::new(),
            pending_file_info_queries: HashMap::new(),
            self_input_muted: None,
            pending_file_lists: HashMap::new(),
            pending_perm_overviews: HashMap::new(),
            pending_stream_items: VecDeque::new(),
            whisper_clients: Vec::new(),
            whisper_channels: Vec::new(),
            audio_sink: None,
            users_snapshot: None,
            channels_snapshot: None,
            reconnect_started: None,
            reconnect_stalled_reported: false,
        }
    }

    #[test]
    fn disconnected_client_rejects_commands() {
        let mut client = disconnected_client();
        assert!(matches!(
            client.send_server_message("x"),
            Err(Error::Connection(ConnectionError::NotConnected))
        ));
        assert!(matches!(
            client.send_audio(&[0u8; 16], AudioCodec::OpusVoice),
            Err(Error::Connection(ConnectionError::NotConnected))
        ));
        assert!(matches!(
            client.move_to_channel(1),
            Err(Error::Connection(ConnectionError::NotConnected))
        ));
        assert!(matches!(
            client.list_files(1, "/"),
            Err(Error::Connection(ConnectionError::NotConnected))
        ));
    }

    #[test]
    fn file_transfer_defaults_are_bounded() {
        let config = FileTransferConfig::default();
        assert_eq!(config.max_memory_download, Some(64 * 1024 * 1024));
        assert_eq!(config.idle_timeout, Some(std::time::Duration::from_secs(30)));
        assert_eq!(config.chunk_size, 64 * 1024);
    }

    #[tokio::test]
    async fn reads_file_in_chunks() {
        let data = vec![7u8; 200_000];
        let (mut reader, mut writer) = tokio::io::duplex(8 * 1024);
        let expected = data.clone();
        let task = tokio::spawn(async move {
            writer.write_all(&expected).await.unwrap();
        });

        let config = FileTransferConfig {
            max_memory_download: None,
            idle_timeout: Some(std::time::Duration::from_secs(1)),
            chunk_size: 4096,
        };
        let read = read_exact_with_idle_timeout(&mut reader, data.len() as u64, &config)
            .await
            .unwrap();
        task.await.unwrap();
        assert_eq!(read, data);
    }

    #[tokio::test]
    async fn read_reports_idle_timeout() {
        let (mut reader, _writer) = tokio::io::duplex(64);
        let config = FileTransferConfig {
            max_memory_download: None,
            idle_timeout: Some(std::time::Duration::from_millis(20)),
            chunk_size: 16,
        };
        let error = read_exact_with_idle_timeout(&mut reader, 16, &config)
            .await
            .unwrap_err();
        assert!(error.contains("idle timeout"), "{error}");
    }

    #[tokio::test]
    async fn writes_file_in_chunks() {
        let data = vec![9u8; 200_000];
        let (mut writer, mut reader) = tokio::io::duplex(8 * 1024);
        let task = tokio::spawn(async move {
            let mut out = Vec::new();
            reader.read_to_end(&mut out).await.unwrap();
            out
        });

        let config = FileTransferConfig {
            max_memory_download: None,
            idle_timeout: Some(std::time::Duration::from_secs(1)),
            chunk_size: 4096,
        };
        write_all_with_idle_timeout(&mut writer, &data, &config)
            .await
            .unwrap();
        drop(writer);
        let written = task.await.unwrap();
        assert_eq!(written, data);
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "tslib-{name}-{}-{nanos}.bin",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn copy_stream_to_file_writes_exact_bytes() {
        let path = temp_path("download");
        let mut input: &[u8] = b"hello world";
        let config = FileTransferConfig::default();

        let written = copy_stream_to_file(&mut input, 11, &path, 0, &config)
            .await
            .unwrap();

        assert_eq!(written, 11);
        assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn copy_stream_to_file_appends_at_offset() {
        let path = temp_path("resume");
        std::fs::write(&path, b"hello ").unwrap();
        let mut input: &[u8] = b"world";
        let config = FileTransferConfig::default();

        copy_stream_to_file(&mut input, 5, &path, 6, &config)
            .await
            .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn copy_file_to_stream_respects_offset() {
        let path = temp_path("upload");
        std::fs::write(&path, b"hello world").unwrap();
        let config = FileTransferConfig::default();
        let (mut writer, mut reader) = tokio::io::duplex(64);

        copy_file_to_stream(&path, 6, 11, &mut writer, &config)
            .await
            .unwrap();
        drop(writer);

        let mut uploaded = Vec::new();
        reader.read_to_end(&mut uploaded).await.unwrap();
        assert_eq!(uploaded, b"world");
        let _ = std::fs::remove_file(&path);
    }
}
