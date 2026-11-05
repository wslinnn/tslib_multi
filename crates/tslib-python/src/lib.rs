mod audio;
mod bbcode;
mod channel_tree;
mod client;
mod error;
mod identity;
mod message;
mod types;

use pyo3::prelude::*;

/// tslib — Python bindings for the TeamSpeak 3 client library.
///
/// Example::
///
///     import tslib
///
///     identity = tslib.Identity()
///     print(f"UID: {identity.unique_id}")
///
///     client = tslib.Client("localhost:9987", identity, "PyBot")
///     client.wait_connected()
///     client.send_server_message("Hello from Python!")
///     client.disconnect()
///
/// Threading::
///
///     The client is unsendable and ``wait_connected()`` / ``process_events()``
///     block the calling thread while holding the GIL. Run the event loop on a
///     dedicated Python thread (or thread the client through a queue) if other
///     threads must stay responsive. A future release will move the connection
///     to a background thread with non-blocking results.
#[pymodule]
fn tslib(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Identity
    m.add_class::<identity::PyIdentity>()?;

    // Client
    m.add_class::<client::PyClient>()?;

    // Types
    m.add_class::<types::PyConnectionState>()?;
    m.add_class::<types::PyChannel>()?;
    m.add_class::<types::PyUser>()?;
    m.add_class::<types::PyServerInfo>()?;

    // BBCode functions
    m.add_function(wrap_pyfunction!(bbcode::bbcode_to_html, m)?)?;
    m.add_function(wrap_pyfunction!(bbcode::bbcode_to_plain, m)?)?;
    m.add_function(wrap_pyfunction!(bbcode::bbcode_to_ansi, m)?)?;
    m.add_function(wrap_pyfunction!(bbcode::py_strip_bbcode, m)?)?;

    // Message builder
    m.add_class::<message::PyMessageBuilder>()?;
    m.add_class::<message::PyChatMessage>()?;
    m.add_class::<message::PyChatHistory>()?;

    // Channel tree
    m.add_class::<channel_tree::PyChannelTree>()?;

    // Audio
    m.add_class::<audio::PyAudioConfig>()?;
    m.add_class::<audio::PyOpusCodec>()?;

    Ok(())
}
