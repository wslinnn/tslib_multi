//! Chat history management

use crate::message::{ChatMessage, MessageTarget};
use std::collections::VecDeque;

/// Chat history storage
pub struct ChatHistory {
    /// Maximum messages to keep per conversation
    max_messages: usize,
    /// Server messages
    server_messages: VecDeque<ChatMessage>,
    /// Channel messages (by channel ID)
    channel_messages: std::collections::HashMap<u64, VecDeque<ChatMessage>>,
    /// Private messages (by user ID)
    private_messages: std::collections::HashMap<u16, VecDeque<ChatMessage>>,
}

impl ChatHistory {
    /// Create a new chat history
    pub fn new(max_messages: usize) -> Self {
        Self {
            max_messages,
            server_messages: VecDeque::with_capacity(max_messages),
            channel_messages: std::collections::HashMap::new(),
            private_messages: std::collections::HashMap::new(),
        }
    }

    /// Add a message to history
    pub fn add(&mut self, message: ChatMessage) {
        let queue = match message.target {
            MessageTarget::Server => &mut self.server_messages,
            MessageTarget::Channel(id) => self
                .channel_messages
                .entry(id)
                .or_insert_with(|| VecDeque::with_capacity(self.max_messages)),
            MessageTarget::Private(id) => self
                .private_messages
                .entry(id)
                .or_insert_with(|| VecDeque::with_capacity(self.max_messages)),
        };

        if queue.len() >= self.max_messages {
            queue.pop_front();
        }
        queue.push_back(message);
    }

    /// Add a server-wide message.
    pub fn add_server_message(
        &mut self,
        sender_id: u16,
        sender_name: impl Into<String>,
        content: impl Into<String>,
    ) {
        self.add(ChatMessage::new(
            sender_id,
            sender_name,
            MessageTarget::Server,
            content,
        ));
    }

    /// Add a message to a channel conversation.
    pub fn add_channel_message(
        &mut self,
        channel_id: u64,
        sender_id: u16,
        sender_name: impl Into<String>,
        content: impl Into<String>,
    ) {
        self.add(ChatMessage::new(
            sender_id,
            sender_name,
            MessageTarget::Channel(channel_id),
            content,
        ));
    }

    /// Add a private message with `user_id` (the other participant).
    pub fn add_private_message(
        &mut self,
        user_id: u16,
        sender_id: u16,
        sender_name: impl Into<String>,
        content: impl Into<String>,
    ) {
        self.add(ChatMessage::new(
            sender_id,
            sender_name,
            MessageTarget::Private(user_id),
            content,
        ));
    }

    /// Get server messages
    pub fn server_messages(&self) -> impl Iterator<Item = &ChatMessage> {
        self.server_messages.iter()
    }

    /// Get channel messages
    pub fn channel_messages(&self, channel_id: u64) -> impl Iterator<Item = &ChatMessage> {
        self.channel_messages
            .get(&channel_id)
            .map(|q| q.iter())
            .into_iter()
            .flatten()
    }

    /// Get private messages with a user
    pub fn private_messages(&self, user_id: u16) -> impl Iterator<Item = &ChatMessage> {
        self.private_messages
            .get(&user_id)
            .map(|q| q.iter())
            .into_iter()
            .flatten()
    }

    /// Get all messages (across all conversations)
    pub fn all_messages(&self) -> impl Iterator<Item = &ChatMessage> {
        self.server_messages
            .iter()
            .chain(self.channel_messages.values().flat_map(|q| q.iter()))
            .chain(self.private_messages.values().flat_map(|q| q.iter()))
    }

    /// Search messages by content
    pub fn search(&self, query: &str) -> Vec<&ChatMessage> {
        let query = query.to_lowercase();
        self.all_messages()
            .filter(|m| m.content.to_lowercase().contains(&query))
            .collect()
    }

    /// Get messages from a specific sender
    pub fn from_sender(&self, sender_id: u16) -> Vec<&ChatMessage> {
        self.all_messages()
            .filter(|m| m.sender_id == sender_id)
            .collect()
    }

    /// Clear all history
    pub fn clear(&mut self) {
        self.server_messages.clear();
        self.channel_messages.clear();
        self.private_messages.clear();
    }

    /// Clear history for a specific channel
    pub fn clear_channel(&mut self, channel_id: u64) {
        self.channel_messages.remove(&channel_id);
    }

    /// Clear history for a specific user
    pub fn clear_user(&mut self, user_id: u16) {
        self.private_messages.remove(&user_id);
    }

    /// Get total message count
    pub fn total_count(&self) -> usize {
        self.server_messages.len()
            + self.channel_messages.values().map(|q| q.len()).sum::<usize>()
            + self.private_messages.values().map(|q| q.len()).sum::<usize>()
    }
}

impl Default for ChatHistory {
    fn default() -> Self {
        Self::new(1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_msg(content: &str) -> ChatMessage {
        ChatMessage::new(1, "Alice", MessageTarget::Server, content)
    }

    fn channel_msg(channel: u64, content: &str) -> ChatMessage {
        ChatMessage::new(1, "Alice", MessageTarget::Channel(channel), content)
    }

    fn private_msg(user: u16, sender: u16, content: &str) -> ChatMessage {
        ChatMessage::new(sender, "Bob", MessageTarget::Private(user), content)
    }

    #[test]
    fn add_and_retrieve_server() {
        let mut h = ChatHistory::new(100);
        h.add(server_msg("hello"));
        assert_eq!(h.server_messages().count(), 1);
        assert_eq!(h.total_count(), 1);
    }

    #[test]
    fn add_and_retrieve_channel() {
        let mut h = ChatHistory::new(100);
        h.add(channel_msg(10, "hi"));
        assert_eq!(h.channel_messages(10).count(), 1);
        assert_eq!(h.channel_messages(99).count(), 0);
    }

    #[test]
    fn add_and_retrieve_private() {
        let mut h = ChatHistory::new(100);
        h.add(private_msg(5, 1, "hey"));
        assert_eq!(h.private_messages(5).count(), 1);
        assert_eq!(h.private_messages(99).count(), 0);
    }

    #[test]
    fn overflow_drops_oldest() {
        let mut h = ChatHistory::new(3);
        h.add(server_msg("a"));
        h.add(server_msg("b"));
        h.add(server_msg("c"));
        h.add(server_msg("d"));
        assert_eq!(h.total_count(), 3);
        let msgs: Vec<_> = h.server_messages().collect();
        assert_eq!(msgs[0].content, "b");
        assert_eq!(msgs[2].content, "d");
    }

    #[test]
    fn all_messages_spans_targets() {
        let mut h = ChatHistory::new(100);
        h.add(server_msg("s"));
        h.add(channel_msg(1, "c"));
        h.add(private_msg(2, 1, "p"));
        assert_eq!(h.all_messages().count(), 3);
    }

    #[test]
    fn convenience_add_methods() {
        let mut h = ChatHistory::new(100);
        h.add_server_message(1, "Alice", "s");
        h.add_channel_message(10, 2, "Bob", "c");
        h.add_private_message(5, 2, "Bob", "p");

        assert_eq!(h.server_messages().count(), 1);
        assert_eq!(h.channel_messages(10).count(), 1);
        assert_eq!(h.private_messages(5).count(), 1);
        assert_eq!(h.total_count(), 3);
    }

    #[test]
    fn search_case_insensitive() {
        let mut h = ChatHistory::new(100);
        h.add(server_msg("Hello World"));
        h.add(server_msg("goodbye"));
        let results = h.search("hello");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Hello World");
    }

    #[test]
    fn search_no_match() {
        let mut h = ChatHistory::new(100);
        h.add(server_msg("hello"));
        assert!(h.search("xyz").is_empty());
    }

    #[test]
    fn from_sender_filter() {
        let mut h = ChatHistory::new(100);
        h.add(ChatMessage::new(1, "A", MessageTarget::Server, "x"));
        h.add(ChatMessage::new(2, "B", MessageTarget::Server, "y"));
        h.add(ChatMessage::new(1, "A", MessageTarget::Server, "z"));
        assert_eq!(h.from_sender(1).len(), 2);
        assert_eq!(h.from_sender(2).len(), 1);
        assert_eq!(h.from_sender(99).len(), 0);
    }

    #[test]
    fn clear_all() {
        let mut h = ChatHistory::new(100);
        h.add(server_msg("s"));
        h.add(channel_msg(1, "c"));
        h.add(private_msg(2, 1, "p"));
        h.clear();
        assert_eq!(h.total_count(), 0);
    }

    #[test]
    fn clear_channel() {
        let mut h = ChatHistory::new(100);
        h.add(channel_msg(1, "a"));
        h.add(channel_msg(2, "b"));
        h.clear_channel(1);
        assert_eq!(h.channel_messages(1).count(), 0);
        assert_eq!(h.channel_messages(2).count(), 1);
    }

    #[test]
    fn clear_user() {
        let mut h = ChatHistory::new(100);
        h.add(private_msg(1, 10, "a"));
        h.add(private_msg(2, 10, "b"));
        h.clear_user(1);
        assert_eq!(h.private_messages(1).count(), 0);
        assert_eq!(h.private_messages(2).count(), 1);
    }

    #[test]
    fn default_capacity() {
        let h = ChatHistory::default();
        // Just verify it doesn't panic and has 0 messages
        assert_eq!(h.total_count(), 0);
    }
}
