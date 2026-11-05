//! Background actor that owns the non-`Send` [`Client`].
//!
//! `Client` cannot be moved between threads, so this module runs it on a
//! dedicated thread with a current-thread Tokio runtime. Callers interact with
//! an [`ActorHandle`], which is `Send + Sync`:
//!
//! - commands are executed on the actor thread and return a result or timeout;
//! - events are fanned out to every subscriber;
//! - control events use an unbounded queue (never dropped), voice events use a
//!   bounded broadcast ring (drop-oldest, counted).
//!
//! ```no_run
//! # async fn example() -> tslib_core::Result<()> {
//! let config = tslib_core::ClientConfig::builder()
//!     .address("localhost:9987")
//!     .identity(tslib_core::Identity::create()?)
//!     .nickname("bot")
//!     .build()?;
//! let actor = tslib_core::ActorHandle::spawn(config)?;
//! let mut events = actor.subscribe();
//! let id = actor.execute(std::time::Duration::from_secs(5), |c| Ok(c.client_id())).await?;
//! while let Some(event) = events.recv().await {
//!     // handle event
//! }
//! # Ok(())
//! # }
//! ```

use crate::client::Client;
use crate::config::ClientConfig;
use crate::connection::ConnectionState;
use crate::error::{ConnectionError, Error, Result};
use crate::events::Event;
use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};

type Job = Box<dyn FnOnce(&mut Client) + Send>;

enum ActorCommand {
    Execute(Job),
    Shutdown,
}

/// Capacity of the per-subscriber voice event ring.
const VOICE_CHANNEL_CAPACITY: usize = 64;

struct Subscriber {
    control: mpsc::UnboundedSender<Event>,
    voice: broadcast::Sender<Event>,
}

struct EventBus {
    subscribers: Mutex<Vec<Subscriber>>,
}

impl EventBus {
    fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
        }
    }

    fn subscribe(&self) -> ActorEventStream {
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let (voice_tx, voice_rx) = broadcast::channel(VOICE_CHANNEL_CAPACITY);
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        subscribers.retain(|sub| !sub.control.is_closed());
        subscribers.push(Subscriber {
            control: control_tx,
            voice: voice_tx.clone(),
        });

        ActorEventStream {
            control: control_rx,
            voice: voice_rx,
            voice_sender: voice_tx,
            dropped_voice: 0,
        }
    }

    fn publish(&self, event: Event) {
        let voice = matches!(
            event,
            Event::AudioReceived { .. }
                | Event::TalkStatusStart { .. }
                | Event::TalkStatusStop { .. }
        );

        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        subscribers.retain(|sub| !sub.control.is_closed());
        for sub in subscribers.iter() {
            if voice {
                // A full broadcast ring drops the oldest entry for slow
                // consumers instead of blocking the pump.
                if sub.voice.send(event.clone()).is_err() {
                    // No receivers: nothing to do.
                }
            } else if sub.control.send(event.clone()).is_err() {
                // Receiver dropped between retain and send.
            }
        }
    }

}

/// Per-subscriber event stream.
pub struct ActorEventStream {
    control: mpsc::UnboundedReceiver<Event>,
    voice: broadcast::Receiver<Event>,
    voice_sender: broadcast::Sender<Event>,
    dropped_voice: u64,
}

impl ActorEventStream {
    /// Wait for the next event.
    ///
    /// Control events are never dropped. Voice events may lag when the
    /// consumer is slow; lagged frames are counted in
    /// [`Self::dropped_voice_events`].
    pub async fn recv(&mut self) -> Option<Event> {
        loop {
            tokio::select! {
                event = self.control.recv() => {
                    if event.is_some() {
                        return event;
                    }
                    // Control channel closed: fall through to voice only.
                    return self.voice.recv().await.ok();
                }
                event = self.voice.recv() => {
                    match event {
                        Ok(event) => return Some(event),
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            self.dropped_voice = self.dropped_voice.saturating_add(count);
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return self.control.recv().await;
                        }
                    }
                }
            }
        }
    }

    /// Wait for the next event or return `None` after `timeout`.
    pub async fn recv_timeout(&mut self, timeout: Duration) -> Option<Event> {
        match tokio::time::timeout(timeout, self.recv()).await {
            Ok(event) => event,
            Err(_) => None,
        }
    }

    /// Non-blocking poll; returns `None` when no event is queued.
    pub fn try_recv(&mut self) -> Option<Event> {
        if let Ok(event) = self.control.try_recv() {
            return Some(event);
        }
        loop {
            match self.voice.try_recv() {
                Ok(event) => return Some(event),
                Err(broadcast::error::TryRecvError::Lagged(count)) => {
                    self.dropped_voice = self.dropped_voice.saturating_add(count);
                    continue;
                }
                Err(broadcast::error::TryRecvError::Empty)
                | Err(broadcast::error::TryRecvError::Closed) => return None,
            }
        }
    }

    /// Voice events dropped for this subscriber because it was too slow.
    pub fn dropped_voice_events(&self) -> u64 {
        self.dropped_voice
    }

    /// Queued control events (unbounded, never dropped).
    pub fn pending_control_events(&self) -> usize {
        self.control.len()
    }

    /// Queued voice events in the bounded broadcast ring.
    pub fn pending_voice_events(&self) -> usize {
        self.voice.len()
    }

    /// Number of live subscribers (including this one).
    pub fn subscriber_count(&self) -> usize {
        self.voice_sender.receiver_count()
    }
}

/// Handle to a running [`Client`] actor.
#[derive(Clone)]
pub struct ActorHandle {
    commands: mpsc::Sender<ActorCommand>,
    bus: Arc<EventBus>,
    state: Arc<Mutex<ConnectionState>>,
}

impl ActorHandle {
    /// Connect and start the actor thread.
    ///
    /// Blocks until the initial connection and state synchronization finish.
    pub fn spawn(config: ClientConfig) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel(64);
        let bus = Arc::new(EventBus::new());
        let state = Arc::new(Mutex::new(ConnectionState::Connecting));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        let bus_for_thread = Arc::clone(&bus);
        let state_for_thread = Arc::clone(&state);

        std::thread::Builder::new()
            .name("tslib-actor".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(Error::Internal(error.to_string())));
                        return;
                    }
                };

                runtime.block_on(async move {
                    let mut client = match Client::connect(config) {
                        Ok(client) => client,
                        Err(error) => {
                            let _ = ready_tx.send(Err(error));
                            return;
                        }
                    };
                    if let Err(error) = client.wait_connected().await {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }

                    *state_for_thread.lock().unwrap_or_else(|e| e.into_inner()) =
                        ConnectionState::Connected;

                    let info = client.server_info();
                    bus_for_thread.publish(Event::Connected {
                        server_name: info.name,
                        welcome_message: info.welcome_message,
                    });
                    let _ = ready_tx.send(Ok(()));

                    actor_loop(&mut client, command_rx, bus_for_thread, state_for_thread).await;
                });
            })
            .map_err(|error| Error::Internal(format!("failed to spawn actor: {error}")))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                commands: command_tx,
                bus,
                state,
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(Error::Internal("actor thread exited during startup".into())),
        }
    }

    /// Subscribe to the actor's event stream.
    pub fn subscribe(&self) -> ActorEventStream {
        self.bus.subscribe()
    }

    /// Execute a closure on the actor thread and wait for its result.
    ///
    /// The closure must be quick; it runs between event pumps. Use
    /// [`Self::shutdown`] to stop the actor.
    pub async fn execute<T, F>(&self, timeout: Duration, job: F) -> Result<T>
    where
        F: FnOnce(&mut Client) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let boxed: Job = Box::new(move |client| {
            let result = job(client).map(|value| Box::new(value) as Box<dyn Any + Send>);
            let _ = reply_tx.send(result);
        });

        self.commands
            .send(ActorCommand::Execute(boxed))
            .await
            .map_err(|_| Error::Connection(ConnectionError::NotConnected))?;

        match tokio::time::timeout(timeout, reply_rx).await {
            Ok(Ok(Ok(value))) => value
                .downcast::<T>()
                .map(|value| *value)
                .map_err(|_| Error::Internal("actor result type mismatch".into())),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(Error::Internal("actor dropped the command result".into())),
            Err(_) => Err(Error::Timeout),
        }
    }

    /// Current connection state, without touching the actor thread.
    pub fn state(&self) -> ConnectionState {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether the actor still accepts commands.
    pub fn is_running(&self) -> bool {
        !self.commands.is_closed()
    }

    /// Our client id (query on the actor thread).
    pub async fn client_id(&self) -> Result<Option<u16>> {
        self.execute(Duration::from_secs(2), |client| Ok(client.client_id()))
            .await
    }

    /// Our channel id (query on the actor thread).
    pub async fn channel_id(&self) -> Result<Option<u64>> {
        self.execute(Duration::from_secs(2), |client| Ok(client.channel_id()))
            .await
    }

    /// Snapshot of all users.
    pub async fn users(&self) -> Result<Arc<[crate::state::User]>> {
        self.execute(Duration::from_secs(5), |client| Ok(client.users()))
            .await
    }

    /// Snapshot of all channels.
    pub async fn channels(&self) -> Result<Arc<[crate::state::Channel]>> {
        self.execute(Duration::from_secs(5), |client| Ok(client.channels()))
            .await
    }

    /// Send a server-wide message and wait for the actor to hand it off.
    pub async fn send_server_message(&self, message: impl Into<String> + Send + 'static) -> Result<()> {
        self.execute(Duration::from_secs(5), move |client| {
            client.send_server_message(message)
        })
        .await
    }

    /// Send a channel message and wait for the actor to hand it off.
    pub async fn send_channel_message(&self, message: impl Into<String> + Send + 'static) -> Result<()> {
        self.execute(Duration::from_secs(5), move |client| {
            client.send_channel_message(message)
        })
        .await
    }

    /// Send a private message and wait for the actor to hand it off.
    pub async fn send_private_message(
        &self,
        user_id: u16,
        message: impl Into<String> + Send + 'static,
    ) -> Result<()> {
        self.execute(Duration::from_secs(5), move |client| {
            client.send_private_message(user_id, message)
        })
        .await
    }

    /// Send a poke and wait for the actor to hand it off.
    pub async fn send_poke(
        &self,
        user_id: u16,
        message: impl Into<String> + Send + 'static,
    ) -> Result<()> {
        self.execute(Duration::from_secs(5), move |client| {
            client.send_poke(user_id, message)
        })
        .await
    }

    /// Move to another channel.
    pub async fn move_to_channel(&self, channel_id: u64) -> Result<()> {
        self.execute(Duration::from_secs(5), move |client| {
            client.move_to_channel(channel_id)
        })
        .await
    }

    /// Update our input mute state.
    pub async fn set_input_muted(&self, muted: bool) -> Result<()> {
        self.execute(Duration::from_secs(5), move |client| {
            client.set_input_muted(muted)
        })
        .await
    }

    /// Disconnect and stop the actor thread.
    pub async fn shutdown(self) -> Result<()> {
        let result = self
            .execute(Duration::from_secs(5), |client| client.disconnect())
            .await;
        let _ = self.commands.send(ActorCommand::Shutdown).await;
        result
    }
}

async fn actor_loop(
    client: &mut Client,
    mut commands: mpsc::Receiver<ActorCommand>,
    bus: Arc<EventBus>,
    state: Arc<Mutex<ConnectionState>>,
) {
    loop {
        // Pump events with a short timeout so commands are handled promptly
        // even on a quiet server.
        match client.wait_events(20).await {
            Ok(events) => {
                for event in events {
                    bus.publish(event);
                }
            }
            Err(error) => {
                if matches!(
                    error,
                    Error::Connection(ConnectionError::NotConnected)
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }

        *state.lock().unwrap_or_else(|e| e.into_inner()) = client.state();

        loop {
            match commands.try_recv() {
                Ok(ActorCommand::Execute(job)) => {
                    job(client);
                }
                Ok(ActorCommand::Shutdown) | Err(mpsc::error::TryRecvError::Disconnected) => {
                    return;
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
            }
        }
    }

    while let Some(command) = commands.recv().await {
        match command {
            ActorCommand::Execute(job) => {
                job(client);
            }
            ActorCommand::Shutdown => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::MessageTarget;

    #[test]
    fn control_events_are_not_dropped() {
        let bus = EventBus::new();
        let mut stream = bus.subscribe();

        for i in 0..200 {
            bus.publish(Event::CommandError {
                error_id: i,
                message: "x".into(),
            });
        }
        // Control events are queued without a capacity limit.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let count = runtime.block_on(async {
            let mut count = 0;
            while let Some(event) = stream.recv_timeout(Duration::from_millis(10)).await {
                count += 1;
                if matches!(event, Event::CommandError { .. }) && count == 200 {
                    break;
                }
            }
            count
        });
        assert_eq!(count, 200);
    }

    #[test]
    fn voice_events_lag_without_blocking() {
        let bus = EventBus::new();
        let mut stream = bus.subscribe();

        for i in 0..(VOICE_CHANNEL_CAPACITY * 2) {
            bus.publish(Event::AudioReceived {
                user_id: 1,
                codec: crate::events::AudioCodec::OpusVoice,
                data: Arc::from(vec![i as u8]),
                seq: i as u16,
                is_whisper: false,
            });
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let _ = stream.recv_timeout(Duration::from_millis(10)).await;
            // Drain until lag is observed.
            for _ in 0..(VOICE_CHANNEL_CAPACITY * 2) {
                let _ = stream.recv_timeout(Duration::from_millis(1)).await;
            }
        });
        assert!(stream.dropped_voice_events() > 0);
    }

    #[test]
    fn private_message_target_is_a_client_id() {
        // Compile-time check that the event shape used by the actor tests is
        // still valid after C1 changes.
        let event = Event::TextMessage {
            sender_id: 1,
            sender_name: "a".into(),
            message: "b".into(),
            target: MessageTarget::Private,
            target_id: Some(7),
        };
        assert!(matches!(
            event,
            Event::TextMessage {
                target_id: Some(7),
                ..
            }
        ));
    }
}
