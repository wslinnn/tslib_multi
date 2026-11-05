use pyo3::prelude::*;
use tslib_chat::message::MessageBuilder;

/// A builder for creating BBCode-formatted messages.
///
/// Example::
///
///     b = MessageBuilder()
///     b.bold("Hello")
///     b.text(" ")
///     b.italic("world")
///     msg = b.build()
#[pyclass(name = "MessageBuilder")]
pub struct PyMessageBuilder {
    inner: MessageBuilder,
}

#[pymethods]
impl PyMessageBuilder {
    #[new]
    fn new() -> Self {
        Self {
            inner: MessageBuilder::new(),
        }
    }

    /// Set the full message content, replacing any previous content.
    fn content(&mut self, text: String) {
        self.inner = std::mem::take(&mut self.inner).content(text);
    }

    /// Append plain text.
    fn text(&mut self, text: String) {
        self.inner = std::mem::take(&mut self.inner).text(text);
    }

    /// Append bold text.
    fn bold(&mut self, text: String) {
        self.inner = std::mem::take(&mut self.inner).bold(text);
    }

    /// Append italic text.
    fn italic(&mut self, text: String) {
        self.inner = std::mem::take(&mut self.inner).italic(text);
    }

    /// Append underlined text.
    fn underline(&mut self, text: String) {
        self.inner = std::mem::take(&mut self.inner).underline(text);
    }

    /// Append colored text.
    fn color(&mut self, color: String, text: String) {
        self.inner = std::mem::take(&mut self.inner).color(color, text);
    }

    /// Append a URL link.
    #[pyo3(signature = (url, text=None))]
    fn url(&mut self, url: String, text: Option<String>) {
        self.inner = std::mem::take(&mut self.inner).url(url, text.as_deref());
    }

    /// Append a newline.
    fn newline(&mut self) {
        self.inner = std::mem::take(&mut self.inner).newline();
    }

    /// Build and return the final message string.
    fn build(&self) -> String {
        self.inner.clone().build()
    }
}

/// A chat message (read-only snapshot).
#[pyclass(name = "ChatMessage", get_all, frozen, from_py_object)]
#[derive(Clone)]
pub struct PyChatMessage {
    pub id: u64,
    pub sender_id: u16,
    pub sender_name: String,
    pub sender_uid: Option<String>,
    pub content: String,
    pub is_own: bool,
}

#[pymethods]
impl PyChatMessage {
    fn __repr__(&self) -> String {
        format!(
            "ChatMessage(id={}, sender='{}', content='{}')",
            self.id, self.sender_name, self.content
        )
    }
}

impl From<&tslib_chat::message::ChatMessage> for PyChatMessage {
    fn from(m: &tslib_chat::message::ChatMessage) -> Self {
        Self {
            id: m.id,
            sender_id: m.sender_id,
            sender_name: m.sender_name.clone(),
            sender_uid: m.sender_uid.clone(),
            content: m.content.clone(),
            is_own: m.is_own,
        }
    }
}

/// Chat history storage.
#[pyclass(name = "ChatHistory", unsendable)]
pub struct PyChatHistory {
    inner: tslib_chat::ChatHistory,
}

#[pymethods]
impl PyChatHistory {
    /// Create a new chat history with a maximum message count.
    #[new]
    #[pyo3(signature = (max_messages=1000))]
    fn new(max_messages: usize) -> Self {
        Self {
            inner: tslib_chat::ChatHistory::new(max_messages),
        }
    }

    /// Get all server messages.
    fn server_messages(&self) -> Vec<PyChatMessage> {
        self.inner.server_messages().map(PyChatMessage::from).collect()
    }

    /// Add a server-wide message to history.
    fn add_server_message(&mut self, sender_id: u16, sender_name: String, content: String) {
        self.inner
            .add_server_message(sender_id, sender_name, content);
    }

    /// Add a channel message to history.
    fn add_channel_message(
        &mut self,
        channel_id: u64,
        sender_id: u16,
        sender_name: String,
        content: String,
    ) {
        self.inner
            .add_channel_message(channel_id, sender_id, sender_name, content);
    }

    /// Add a private message with ``user_id`` to history.
    fn add_private_message(
        &mut self,
        user_id: u16,
        sender_id: u16,
        sender_name: String,
        content: String,
    ) {
        self.inner
            .add_private_message(user_id, sender_id, sender_name, content);
    }

    /// Get channel messages by channel ID.
    fn channel_messages(&self, channel_id: u64) -> Vec<PyChatMessage> {
        self.inner
            .channel_messages(channel_id)
            .map(PyChatMessage::from)
            .collect()
    }

    /// Get private messages with a user.
    fn private_messages(&self, user_id: u16) -> Vec<PyChatMessage> {
        self.inner
            .private_messages(user_id)
            .map(PyChatMessage::from)
            .collect()
    }

    /// Search messages by content substring.
    fn search(&self, query: String) -> Vec<PyChatMessage> {
        self.inner.search(&query).into_iter().map(PyChatMessage::from).collect()
    }

    /// Clear all history.
    fn clear(&mut self) {
        self.inner.clear();
    }

    /// Get total message count.
    fn total_count(&self) -> usize {
        self.inner.total_count()
    }
}
