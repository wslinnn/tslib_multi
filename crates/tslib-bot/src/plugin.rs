//! Plugin system

use crate::bot::Bot;
use crate::command::Command;
use crate::error::Result;
use async_trait::async_trait;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Plugin trait
#[async_trait]
pub trait Plugin: Send + Sync {
    /// Plugin name
    fn name(&self) -> &str;

    /// Plugin version
    fn version(&self) -> &str {
        "1.0.0"
    }

    /// Plugin description
    fn description(&self) -> &str {
        ""
    }

    /// Called when the plugin is loaded
    async fn on_load(&mut self, _bot: &Bot) -> Result<()> {
        Ok(())
    }

    /// Called when the plugin is unloaded
    async fn on_unload(&mut self) -> Result<()> {
        Ok(())
    }

    /// Get commands provided by this plugin
    fn commands(&self) -> Vec<Command> {
        Vec::new()
    }

    /// Called when connected to server
    async fn on_connect(&self, _bot: &Bot) -> Result<()> {
        Ok(())
    }

    /// Called when disconnected from server
    async fn on_disconnect(&self, _bot: &Bot, _reason: &str) -> Result<()> {
        Ok(())
    }

    /// Cast to Any for downcasting
    fn as_any(&self) -> &dyn Any;

    /// Cast to Any mut for downcasting
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Plugin manager
pub struct PluginManager {
    plugins: HashMap<String, Arc<RwLock<Box<dyn Plugin>>>>,
}

impl PluginManager {
    /// Create a new plugin manager
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    /// Load a plugin
    pub async fn load(&mut self, plugin: Box<dyn Plugin>, bot: &Bot) -> Result<()> {
        let name = plugin.name().to_string();
        let mut plugin = plugin;

        plugin.on_load(bot).await?;

        // Register the commands the plugin declares. `on_load` may also have
        // registered some directly; duplicates are resolved by the registry.
        for command in plugin.commands() {
            bot.register_command(command).await;
        }

        self.plugins.insert(name, Arc::new(RwLock::new(plugin)));
        Ok(())
    }

    /// Unload a plugin
    pub async fn unload(&mut self, name: &str) -> Result<Option<Box<dyn Plugin>>> {
        if let Some(plugin) = self.plugins.remove(name) {
            let mut plugin = Arc::try_unwrap(plugin)
                .map_err(|_| crate::error::BotError::Plugin("Plugin still in use".into()))?
                .into_inner();

            plugin.on_unload().await?;
            Ok(Some(plugin))
        } else {
            Ok(None)
        }
    }

    /// Get a plugin by name
    pub fn get(&self, name: &str) -> Option<Arc<RwLock<Box<dyn Plugin>>>> {
        self.plugins.get(name).cloned()
    }

    /// Get all plugin names
    pub fn names(&self) -> Vec<&str> {
        self.plugins.keys().map(|s| s.as_str()).collect()
    }

    /// Get all commands from all plugins
    pub fn all_commands(&self) -> Vec<Command> {
        // Note: This is synchronous, so we can't use async here
        // Commands should be cached when plugins are loaded
        Vec::new()
    }

    /// Notify all plugins of connection
    pub async fn notify_connect(&self, bot: &Bot) -> Result<()> {
        for plugin in self.plugins.values() {
            plugin.read().await.on_connect(bot).await?;
        }
        Ok(())
    }

    /// Notify all plugins of disconnection
    pub async fn notify_disconnect(&self, bot: &Bot, reason: &str) -> Result<()> {
        for plugin in self.plugins.values() {
            plugin.read().await.on_disconnect(bot, reason).await?;
        }
        Ok(())
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Macro to simplify plugin creation
#[macro_export]
macro_rules! plugin {
    ($name:expr, $version:expr, $($commands:expr),* $(,)?) => {
        {
            struct GeneratedPlugin;

            #[async_trait::async_trait]
            impl $crate::plugin::Plugin for GeneratedPlugin {
                fn name(&self) -> &str { $name }
                fn version(&self) -> &str { $version }

                fn commands(&self) -> Vec<$crate::command::Command> {
                    vec![$($commands),*]
                }

                fn as_any(&self) -> &dyn std::any::Any { self }
                fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
            }

            Box::new(GeneratedPlugin) as Box<dyn $crate::plugin::Plugin>
        }
    };
}
