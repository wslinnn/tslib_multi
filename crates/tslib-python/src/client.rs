use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::time::Duration;

use crate::error::to_py_err;
use crate::identity::PyIdentity;
use crate::types::{connection_state_to_u8, event_to_dict, PyChannel, PyServerInfo, PyUser};

/// A TeamSpeak 3 client connection.
///
/// The client runs on a dedicated native thread (actor) so Python handlers and
/// other Python threads are not blocked by the protocol event pump. Methods
/// that send commands wait for the actor to hand the command off, with a
/// bounded timeout.
///
/// Example::
///
///     client = tslib.Client("localhost:9987", identity, "MyBot")
///     client.send_server_message("Hello!")
///     for event in client.process_events():
///         ...
///     client.disconnect()
#[pyclass(name = "Client", unsendable)]
pub struct PyClient {
    actor: tslib_core::ActorHandle,
    events: tslib_core::ActorEventStream,
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
    ///
    /// The constructor blocks until the initial connection and state
    /// synchronization complete (bounded by ``connect_timeout``).
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
        let actor = tslib_core::ActorHandle::spawn(config).map_err(to_py_err)?;
        let events = actor.subscribe();

        Ok(Self {
            actor,
            events,
            runtime,
        })
    }

    /// Connection is established in the constructor; this is kept for
    /// compatibility and returns immediately.
    fn wait_connected(&mut self) -> PyResult<()> {
        Ok(())
    }

    /// Drain pending events without blocking.
    ///
    /// Returns a list of event dicts. The actor thread keeps pumping in the
    /// background, so this never blocks on the network.
    fn process_events<'py>(&mut self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let mut events = Vec::new();
        while let Some(event) = self.events.try_recv() {
            events.push(event_to_dict(py, &event));
        }
        Ok(events)
    }

    /// Wait up to ``timeout_ms`` for events, then drain the queue.
    #[pyo3(signature = (timeout_ms=100))]
    fn wait_events<'py>(
        &mut self,
        py: Python<'py>,
        timeout_ms: u64,
    ) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let first = self
            .runtime
            .block_on(self.events.recv_timeout(Duration::from_millis(timeout_ms)));
        let mut events = Vec::new();
        if let Some(event) = first {
            events.push(event_to_dict(py, &event));
        }
        while let Some(event) = self.events.try_recv() {
            events.push(event_to_dict(py, &event));
        }
        Ok(events)
    }

    /// Disconnect from the server and stop the actor thread.
    fn disconnect(&mut self) -> PyResult<()> {
        let actor = self.actor.clone();
        self.runtime
            .block_on(actor.shutdown())
            .map_err(to_py_err)
    }

    /// Whether the client is currently connected.
    #[getter]
    fn is_connected(&self) -> bool {
        self.actor.state().is_connected()
    }

    /// The current connection state as an integer.
    ///
    /// Compare with ``ConnectionState`` constants.
    #[getter]
    fn state(&self) -> u8 {
        connection_state_to_u8(self.actor.state())
    }

    /// Our client ID on the server, or None if not yet assigned.
    #[getter]
    fn client_id(&self) -> PyResult<Option<u16>> {
        self.runtime
            .block_on(self.actor.client_id())
            .map_err(to_py_err)
    }

    /// Our current channel ID, or None.
    #[getter]
    fn channel_id(&self) -> PyResult<Option<u64>> {
        self.runtime
            .block_on(self.actor.channel_id())
            .map_err(to_py_err)
    }

    /// Get all channels on the server.
    fn channels(&mut self) -> PyResult<Vec<PyChannel>> {
        let channels = self
            .runtime
            .block_on(self.actor.channels())
            .map_err(to_py_err)?;
        Ok(channels.iter().cloned().map(PyChannel::from).collect())
    }

    /// Get all connected users.
    fn users(&mut self) -> PyResult<Vec<PyUser>> {
        let users = self
            .runtime
            .block_on(self.actor.users())
            .map_err(to_py_err)?;
        Ok(users.iter().cloned().map(PyUser::from).collect())
    }

    /// Get a specific channel by ID.
    fn channel(&mut self, id: u64) -> PyResult<Option<PyChannel>> {
        let channel = self
            .runtime
            .block_on(
                self.actor
                    .execute(Duration::from_secs(5), move |client| Ok(client.channel(id))),
            )
            .map_err(to_py_err)?;
        Ok(channel.map(PyChannel::from))
    }

    /// Get a specific user by ID.
    fn user(&mut self, id: u16) -> PyResult<Option<PyUser>> {
        let user = self
            .runtime
            .block_on(
                self.actor
                    .execute(Duration::from_secs(5), move |client| Ok(client.user(id))),
            )
            .map_err(to_py_err)?;
        Ok(user.map(PyUser::from))
    }

    /// Get live server information.
    fn server_info(&self) -> PyResult<PyServerInfo> {
        let info = self
            .runtime
            .block_on(
                self.actor
                    .execute(Duration::from_secs(5), |client| Ok(client.server_info())),
            )
            .map_err(to_py_err)?;
        Ok(PyServerInfo::from(info))
    }

    /// Send a message to the entire server.
    fn send_server_message(&mut self, msg: String) -> PyResult<()> {
        self.runtime
            .block_on(self.actor.send_server_message(msg))
            .map_err(to_py_err)
    }

    /// Send a message to the current channel.
    fn send_channel_message(&mut self, msg: String) -> PyResult<()> {
        self.runtime
            .block_on(self.actor.send_channel_message(msg))
            .map_err(to_py_err)
    }

    /// Send a private message to a user.
    fn send_private_message(&mut self, user_id: u16, msg: String) -> PyResult<()> {
        self.runtime
            .block_on(self.actor.send_private_message(user_id, msg))
            .map_err(to_py_err)
    }

    /// Move to a different channel.
    fn move_to_channel(&mut self, channel_id: u64) -> PyResult<()> {
        self.runtime
            .block_on(self.actor.move_to_channel(channel_id))
            .map_err(to_py_err)
    }

    /// Re-synchronize the local state from the server.
    fn sync_state(&mut self) -> PyResult<()> {
        self.runtime
            .block_on(
                self.actor
                    .execute(Duration::from_secs(10), |client| client.sync_state()),
            )
            .map_err(to_py_err)
    }
}
