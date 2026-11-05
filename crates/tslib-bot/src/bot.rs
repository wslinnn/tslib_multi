//! Main bot implementation

use crate::command::{Command, CommandContext, CommandRegistry, FnHandler};
use crate::config::BotConfig;
use crate::error::{BotError, Result};
use crate::plugin::{Plugin, PluginManager};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use tslib_core::{Client, ClientConfig, Event};

/// A reply message queued by a command handler
struct ReplyMessage {
    /// Target client ID (for private messages)
    sender_id: u16,
    /// The message text
    message: String,
    /// Whether to reply privately
    is_private: bool,
}

/// TeamSpeak bot
pub struct Bot {
    /// Configuration
    config: BotConfig,
    /// Core client
    client: Option<Client>,
    /// Command registry
    commands: Arc<RwLock<CommandRegistry>>,
    /// Plugin manager
    plugins: Arc<RwLock<PluginManager>>,
    /// Is running
    running: Arc<RwLock<bool>>,
}

impl Bot {
    /// Create a new bot
    pub async fn new(config: BotConfig) -> Result<Self> {
        let commands = Arc::new(RwLock::new(CommandRegistry::new(&config.command_prefix)));

        let mut bot = Self {
            config,
            client: None,
            commands,
            plugins: Arc::new(RwLock::new(PluginManager::new())),
            running: Arc::new(RwLock::new(false)),
        };

        // Register built-in commands
        bot.register_builtin_commands().await;

        Ok(bot)
    }

    /// Connect to the server
    pub async fn connect(&mut self) -> Result<()> {
        let mut builder = ClientConfig::builder()
            .address(&self.config.address)
            .identity(self.config.identity.clone())
            .nickname(&self.config.nickname);
        if let Some(password) = &self.config.password {
            builder = builder.password(password.clone());
        }
        if let Some(channel) = &self.config.channel {
            builder = builder.channel(channel.clone());
        }
        if let Some(channel_password) = &self.config.channel_password {
            builder = builder.channel_password(channel_password.clone());
        }
        let core_config = builder.build().map_err(BotError::Core)?;

        let client = Client::connect(core_config)?;
        self.client = Some(client);

        info!("Bot connected to {}", self.config.address);
        Ok(())
    }

    /// Disconnect from the server
    pub async fn disconnect(&mut self) -> Result<()> {
        if let Some(mut client) = self.client.take() {
            client.disconnect()?;
        }
        Ok(())
    }

    /// Reconnect to the server
    async fn reconnect(&mut self) -> Result<()> {
        // Drop existing connection
        self.client.take();

        // Create a new connection
        self.connect().await?;

        // Wait for the new connection to be established
        if let Some(client) = self.client.as_mut() {
            client.wait_connected().await.map_err(BotError::Core)?;
        }

        Ok(())
    }

    /// Register a command
    pub async fn command<F, Fut>(&self, name: &str, handler: F)
    where
        F: Fn(CommandContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let cmd = Command::new(name, FnHandler::new(handler));
        self.commands.write().await.register(cmd);
    }

    /// Register a command with full options
    pub async fn register_command(&self, command: Command) {
        self.commands.write().await.register(command);
    }

    /// Load a plugin
    pub async fn load_plugin(&self, plugin: Box<dyn Plugin>) -> Result<()> {
        self.plugins.write().await.load(plugin, self).await
    }

    /// Run the bot (blocking)
    pub async fn run(&mut self) -> Result<()> {
        if self.client.is_none() {
            self.connect().await?;
        }

        *self.running.write().await = true;

        // Wait for connection to be fully established
        {
            let client = self.client.as_mut().unwrap();
            info!("Waiting for connection...");
            client.wait_connected().await.map_err(BotError::Core)?;
        }

        // Notify plugins that the connection is ready.
        {
            let plugins = self.plugins.clone();
            plugins.read().await.notify_connect(self).await?;
        }

        let commands = self.commands.clone();
        let config = self.config.clone();

        // Channel for command replies back to the main loop
        let (reply_tx, mut reply_rx) = mpsc::channel::<ReplyMessage>(64);

        // Ctrl+C must not cancel `wait_events` mid-poll, otherwise a collected
        // stream item can be dropped. A tiny flag task avoids that.
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_signal = shutdown.clone();
        let ctrl_c = tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown_signal.store(true, Ordering::SeqCst);
        });

        info!("Bot is running. Press Ctrl+C to stop.");

        while *self.running.read().await && !shutdown.load(Ordering::SeqCst) {
            // Drive the connection event stream. Without this call no server
            // event is ever produced, so the bot would never see messages.
            let events = match self.client.as_mut() {
                Some(client) => match client.wait_events(50).await {
                    Ok(events) => events,
                    Err(e) => {
                        error!("Event pump failed: {}", e);
                        break;
                    }
                },
                None => break,
            };

            for event in events {
                match event {
                    Event::TextMessage {
                        sender_id,
                        sender_name,
                        message,
                        target,
                        ..
                    } => {
                        let is_private =
                            matches!(target, tslib_core::events::MessageTarget::Private);
                        let sender_uid = self
                            .client
                            .as_ref()
                            .and_then(|client| client.user(sender_id))
                            .map(|user| user.uid)
                            .filter(|uid| !uid.is_empty());
                        Self::handle_message(
                            &commands,
                            &config,
                            reply_tx.clone(),
                            sender_id,
                            sender_name,
                            sender_uid,
                            message,
                            is_private,
                        )
                        .await;
                    }
                    Event::Disconnected { reason } => {
                        warn!("Disconnected: {}", reason);
                        if config.auto_reconnect {
                            info!("Reconnecting in {:?}...", config.reconnect_delay);
                            tokio::time::sleep(config.reconnect_delay).await;
                            if let Err(e) = self.reconnect().await {
                                error!("Reconnection failed: {}", e);
                                break;
                            }
                            let plugins = self.plugins.clone();
                            if let Err(e) = plugins.read().await.notify_connect(self).await {
                                error!("Plugin on_connect failed after reconnect: {}", e);
                            }
                            info!("Reconnected successfully");
                        } else {
                            break;
                        }
                    }
                    _ => {}
                }
            }

            // Process pending replies from command handlers
            while let Ok(reply) = reply_rx.try_recv() {
                if let Some(client) = self.client.as_mut() {
                    let result = if reply.is_private {
                        client.send_private_message(reply.sender_id, &reply.message)
                    } else {
                        client.send_channel_message(&reply.message)
                    };
                    if let Err(e) = result {
                        error!("Failed to send reply: {}", e);
                    }
                }
            }
        }

        ctrl_c.abort();

        let plugins = self.plugins.clone();
        if let Err(e) = plugins
            .read()
            .await
            .notify_disconnect(self, "Bot stopped")
            .await
        {
            warn!("Plugin on_disconnect failed: {}", e);
        }

        self.disconnect().await?;
        Ok(())
    }

    /// Stop the bot
    pub async fn stop(&self) {
        *self.running.write().await = false;
    }

    /// Check if user is an owner
    pub fn is_owner(&self, uid: &str) -> bool {
        self.config.owners.iter().any(|o| o == uid)
    }

    /// Get the core client
    pub fn client(&self) -> Option<&Client> {
        self.client.as_ref()
    }

    async fn handle_message(
        commands: &Arc<RwLock<CommandRegistry>>,
        config: &BotConfig,
        reply_tx: mpsc::Sender<ReplyMessage>,
        sender_id: u16,
        sender_name: String,
        sender_uid: Option<String>,
        message: String,
        is_private: bool,
    ) {
        let registry = commands.read().await;

        if let Some((cmd_name, args)) = registry.parse(&message) {
            let cmd_name_owned = cmd_name.to_string();
            if let Some(command) = registry.get(&cmd_name_owned) {
                // Check owner-only
                if command.owner_only {
                    if let Some(ref uid) = sender_uid {
                        if !config.owners.contains(uid) {
                            debug!("User {} tried to use owner-only command {}", sender_name, cmd_name_owned);
                            return;
                        }
                    } else {
                        return;
                    }
                }

                // Create a reply function that sends through the channel
                let reply_sender = reply_tx.clone();
                let reply_target_id = sender_id;
                let reply_is_private = is_private;

                let ctx = CommandContext::new(
                    sender_id,
                    sender_name.clone(),
                    sender_uid,
                    cmd_name_owned.clone(),
                    args,
                    message,
                    is_private,
                    move |msg: String| {
                        let tx = reply_sender.clone();
                        Box::pin(async move {
                            tx.send(ReplyMessage {
                                sender_id: reply_target_id,
                                message: msg,
                                is_private: reply_is_private,
                            })
                            .await
                            .map_err(|e| BotError::Command(format!("Failed to queue reply: {}", e)))?;
                            Ok(())
                        })
                    },
                );

                // Check can_use
                if !command.handler.can_use(&ctx) {
                    debug!("User {} cannot use command {}", sender_name, cmd_name_owned);
                    return;
                }

                // Execute command
                match command.handler.handle(ctx).await {
                    Ok(()) => {
                        debug!("Command {} executed successfully", cmd_name_owned);
                    }
                    Err(e) => {
                        error!("Command {} failed: {}", cmd_name_owned, e);
                    }
                }
            }
        }
    }

    async fn register_builtin_commands(&mut self) {
        // Help command
        let commands = self.commands.clone();
        self.command("help", move |ctx| {
            let commands = commands.clone();
            async move {
                let registry = commands.read().await;
                let cmds = registry.visible_commands();

                let mut help_text = String::from("Available commands:\n");
                for cmd in cmds {
                    help_text.push_str(&format!("  {} - {}\n",
                        cmd.name,
                        cmd.help.as_deref().unwrap_or("No description")
                    ));
                }

                ctx.reply(help_text).await
            }
        }).await;

        // Ping command
        self.command("ping", |ctx| async move {
            ctx.reply("Pong!").await
        }).await;

        // Version command
        self.command("version", |ctx| async move {
            ctx.reply(format!("tslib-bot v{}", env!("CARGO_PKG_VERSION"))).await
        }).await;
    }
}
