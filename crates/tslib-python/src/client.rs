use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::error::to_py_err;
use crate::identity::PyIdentity;
use crate::types::{connection_state_to_u8, event_to_dict, PyChannel, PyServerInfo, PyUser};

/// A TeamSpeak 3 client connection.
///
/// The client is not thread-safe (unsendable) because the underlying
/// tsclientlib connection is not Send/Sync.
///
/// Example::
///
///     client = tslib.Client("localhost:9987", identity, "MyBot")
///     client.wait_connected()
///     client.send_server_message("Hello!")
///     client.disconnect()
#[pyclass(name = "Client", unsendable)]
pub struct PyClient {
    inner: tslib_core::Client,
    runtime: tokio::runtime::Runtime,
}

#[pymethods]
impl PyClient {
    /// Connect to a TeamSpeak server.
    ///
    /// Args:
    ///     address: Server address (host:port).
    ///     identity: The identity to authenticate with.
    ///     nickname: Display name on the server.
    ///     password: Server password (optional).
    ///     channel: Default channel to join (optional).
    #[new]
    #[pyo3(signature = (address, identity, nickname, password=None, channel=None))]
    fn new(
        address: String,
        identity: &PyIdentity,
        nickname: String,
        password: Option<String>,
        channel: Option<String>,
    ) -> PyResult<Self> {
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let mut builder = tslib_core::ClientConfig::builder()
            .address(address)
            .identity(identity.inner.clone())
            .nickname(nickname);

        if let Some(pw) = password {
            builder = builder.password(pw);
        }
        if let Some(ch) = channel {
            builder = builder.channel(ch);
        }

        let config = builder.build().map_err(to_py_err)?;

        let inner = runtime.block_on(async {
            tslib_core::Client::connect(config)
        }).map_err(to_py_err)?;

        Ok(Self { inner, runtime })
    }

    /// Wait for the connection to be fully established.
    ///
    /// This blocks until the initial state synchronization is complete.
    /// It holds the GIL while blocking; see the module docstring.
    fn wait_connected(&mut self) -> PyResult<()> {
        self.runtime
            .block_on(self.inner.wait_connected())
            .map_err(to_py_err)
    }

    /// Process pending events from the server.
    ///
    /// Returns a list of event dicts. Call this regularly to stay in sync.
    /// This blocks the calling thread and holds the GIL while it waits for
    /// the connection stream.
    fn process_events<'py>(&mut self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let events = self
            .runtime
            .block_on(self.inner.process_events())
            .map_err(to_py_err)?;

        Ok(events.iter().map(|e| event_to_dict(py, e)).collect())
    }

    /// Disconnect from the server.
    fn disconnect(&mut self) -> PyResult<()> {
        self.inner.disconnect().map_err(to_py_err)
    }

    /// Whether the client is currently connected.
    #[getter]
    fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    /// The current connection state as an integer.
    ///
    /// Compare with ``ConnectionState`` constants.
    #[getter]
    fn state(&self) -> u8 {
        connection_state_to_u8(self.inner.state())
    }

    /// Our client ID on the server, or None if not yet assigned.
    #[getter]
    fn client_id(&self) -> Option<u16> {
        self.inner.client_id()
    }

    /// Our current channel ID, or None.
    #[getter]
    fn channel_id(&self) -> Option<u64> {
        self.inner.channel_id()
    }

    /// Get all channels on the server.
    fn channels(&mut self) -> Vec<PyChannel> {
        self.inner
            .channels()
            .iter()
            .cloned()
            .map(PyChannel::from)
            .collect()
    }

    /// Get all connected users.
    fn users(&mut self) -> Vec<PyUser> {
        self.inner
            .users()
            .iter()
            .cloned()
            .map(PyUser::from)
            .collect()
    }

    /// Get a specific channel by ID.
    fn channel(&self, id: u64) -> Option<PyChannel> {
        self.inner.channel(id).map(PyChannel::from)
    }

    /// Get a specific user by ID.
    fn user(&self, id: u16) -> Option<PyUser> {
        self.inner.user(id).map(PyUser::from)
    }

    /// Get live server information.
    ///
    /// Reads the current bookkeeping state (online clients/channels, uptime)
    /// instead of returning a cached snapshot from the last sync.
    fn server_info(&self) -> PyServerInfo {
        PyServerInfo::from(self.inner.server_info())
    }

    /// Send a message to the entire server.
    fn send_server_message(&mut self, msg: String) -> PyResult<()> {
        self.inner.send_server_message(msg).map_err(to_py_err)
    }

    /// Send a message to the current channel.
    fn send_channel_message(&mut self, msg: String) -> PyResult<()> {
        self.inner.send_channel_message(msg).map_err(to_py_err)
    }

    /// Send a private message to a user.
    fn send_private_message(&mut self, user_id: u16, msg: String) -> PyResult<()> {
        self.inner
            .send_private_message(user_id, msg)
            .map_err(to_py_err)
    }

    /// Move to a different channel.
    fn move_to_channel(&mut self, channel_id: u64) -> PyResult<()> {
        self.inner.move_to_channel(channel_id).map_err(to_py_err)
    }

    /// Re-synchronize the local state from the server.
    fn sync_state(&mut self) -> PyResult<()> {
        self.inner.sync_state().map_err(to_py_err)
    }
}
