//! # tslib-ffi
//!
//! FFI bindings for tslib providing C-compatible interface
//! for integration with other languages (Python, Java, Go, etc.)
//!
//! ## Safety
//!
//! All functions in this module are `unsafe` as they deal with
//! raw pointers from C code. Users must ensure:
//! - Pointers are valid and properly aligned
//! - Strings are null-terminated UTF-8
//! - Returned pointers are freed using the appropriate free functions
//!
//! ## Thread Safety
//!
//! The client is NOT thread-safe. All calls to a client must be made from
//! the same thread that created it.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;

use tslib_core::{Client, ClientConfig, ConnectionState, Event, Identity};

mod actor;

/// Error codes
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsLibError {
    Ok = 0,
    InvalidArgument = 1,
    ConnectionFailed = 2,
    NotConnected = 3,
    Timeout = 4,
    PermissionDenied = 5,
    ChannelError = 6,
    IdentityError = 7,
    InternalError = 99,
}

/// Opaque handle to a client
#[repr(C)]
pub struct TsLibClient {
    _private: [u8; 0],
}

/// Opaque handle to an actor-backed client
#[repr(C)]
pub struct TsLibActor {
    _private: [u8; 0],
}

pub(crate) fn map_connection_state(state: ConnectionState) -> TsLibConnectionState {
    match state {
        ConnectionState::Disconnected => TsLibConnectionState::Disconnected,
        ConnectionState::Connecting => TsLibConnectionState::Connecting,
        ConnectionState::Connected => TsLibConnectionState::Connected,
        ConnectionState::Initializing => TsLibConnectionState::Connecting,
        ConnectionState::Reconnecting => TsLibConnectionState::Reconnecting,
    }
}

/// Opaque handle to an identity
#[repr(C)]
pub struct TsLibIdentity {
    _private: [u8; 0],
}

/// Connection state
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsLibConnectionState {
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
    Reconnecting = 3,
}

/// Callback for text messages
pub type TextMessageCallback = extern "C" fn(
    sender_id: u16,
    sender_name: *const c_char,
    message: *const c_char,
    is_private: c_int,
    user_data: *mut c_void,
);

/// Callback for connection state changes
pub type ConnectionStateCallback = extern "C" fn(
    old_state: TsLibConnectionState,
    new_state: TsLibConnectionState,
    user_data: *mut c_void,
);

/// Internal FFI client: owns the connection and a runtime used to poll it.
struct FfiClient {
    client: Client,
    runtime: tokio::runtime::Runtime,
    text_callback: Option<TextMessageCallback>,
    state_callback: Option<ConnectionStateCallback>,
    user_data: *mut c_void,
}

// ============================================================================
// Identity functions
// ============================================================================

/// Create a new identity
///
/// # Safety
/// Returns a pointer that must be freed with `tslib_identity_free`
#[no_mangle]
pub unsafe extern "C" fn tslib_identity_create() -> *mut TsLibIdentity {
    match Identity::create() {
        Ok(identity) => Box::into_raw(Box::new(identity)) as *mut TsLibIdentity,
        Err(_) => ptr::null_mut(),
    }
}

/// Load an identity from a file
///
/// # Safety
/// - `path` must be a valid null-terminated UTF-8 string
/// - Returns a pointer that must be freed with `tslib_identity_free`
#[no_mangle]
pub unsafe extern "C" fn tslib_identity_load(path: *const c_char) -> *mut TsLibIdentity {
    if path.is_null() {
        return ptr::null_mut();
    }

    let path = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };

    match Identity::load(path) {
        Ok(identity) => Box::into_raw(Box::new(identity)) as *mut TsLibIdentity,
        Err(_) => ptr::null_mut(),
    }
}

/// Save an identity to a file
///
/// # Safety
/// - `identity` must be a valid pointer from `tslib_identity_create` or `tslib_identity_load`
/// - `path` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn tslib_identity_save(
    identity: *const TsLibIdentity,
    path: *const c_char,
) -> TsLibError {
    if identity.is_null() || path.is_null() {
        return TsLibError::InvalidArgument;
    }

    let identity = &*(identity as *const Identity);
    let path = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return TsLibError::InvalidArgument,
    };

    match identity.save(path) {
        Ok(()) => TsLibError::Ok,
        Err(_) => TsLibError::IdentityError,
    }
}

/// Get the unique ID of an identity
///
/// # Safety
/// - `identity` must be a valid pointer
/// - Returns a string that must be freed with `tslib_string_free`
#[no_mangle]
pub unsafe extern "C" fn tslib_identity_unique_id(
    identity: *const TsLibIdentity,
) -> *mut c_char {
    if identity.is_null() {
        return ptr::null_mut();
    }

    let identity = &*(identity as *const Identity);
    match CString::new(identity.unique_id()) {
        Ok(s) => s.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// Get the security level of an identity
#[no_mangle]
pub unsafe extern "C" fn tslib_identity_security_level(
    identity: *const TsLibIdentity,
) -> c_int {
    if identity.is_null() {
        return -1;
    }

    let identity = &*(identity as *const Identity);
    identity.security_level() as c_int
}

/// Free an identity
///
/// # Safety
/// `identity` must be a valid pointer from `tslib_identity_create` or `tslib_identity_load`
#[no_mangle]
pub unsafe extern "C" fn tslib_identity_free(identity: *mut TsLibIdentity) {
    if !identity.is_null() {
        drop(Box::from_raw(identity as *mut Identity));
    }
}

// ============================================================================
// Client functions
// ============================================================================

/// Connect to a TeamSpeak server
///
/// # Safety
/// - All string parameters must be valid null-terminated UTF-8 strings
/// - `identity` must be a valid pointer (ownership is NOT transferred)
/// - Returns a pointer that must be freed with `tslib_client_free`
/// - The client is NOT thread-safe - all operations must happen on the same thread
#[no_mangle]
pub unsafe extern "C" fn tslib_client_connect(
    address: *const c_char,
    identity: *const TsLibIdentity,
    nickname: *const c_char,
    password: *const c_char,
) -> *mut TsLibClient {
    if address.is_null() || identity.is_null() || nickname.is_null() {
        return ptr::null_mut();
    }

    let address = match CStr::from_ptr(address).to_str() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };

    let identity = (*(identity as *const Identity)).clone();

    let nickname = match CStr::from_ptr(nickname).to_str() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };

    let password = if password.is_null() {
        None
    } else {
        CStr::from_ptr(password).to_str().ok()
    };

    let config = ClientConfig::builder()
        .address(address)
        .identity(identity)
        .nickname(nickname);

    let config = if let Some(pwd) = password {
        config.password(pwd)
    } else {
        config
    };

    let config = match config.build() {
        Ok(c) => c,
        Err(_) => return ptr::null_mut(),
    };

    // Client::connect is synchronous now
    match Client::connect(config) {
        Ok(client) => {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return ptr::null_mut(),
            };
            Box::into_raw(Box::new(FfiClient {
                client,
                runtime,
                text_callback: None,
                state_callback: None,
                user_data: ptr::null_mut(),
            })) as *mut TsLibClient
        }
        Err(_) => ptr::null_mut(),
    }
}

/// Disconnect from the server
///
/// # Safety
/// - `client` must be a valid mutable pointer
#[no_mangle]
pub unsafe extern "C" fn tslib_client_disconnect(client: *mut TsLibClient) -> TsLibError {
    if client.is_null() {
        return TsLibError::InvalidArgument;
    }

    let client = &mut (*(client as *mut FfiClient)).client;

    match client.disconnect() {
        Ok(()) => TsLibError::Ok,
        Err(_) => TsLibError::InternalError,
    }
}

/// Get connection state
#[no_mangle]
pub unsafe extern "C" fn tslib_client_state(client: *const TsLibClient) -> TsLibConnectionState {
    if client.is_null() {
        return TsLibConnectionState::Disconnected;
    }

    let client = &(*(client as *const FfiClient)).client;
    let state = client.state();

    match state {
        tslib_core::ConnectionState::Disconnected => TsLibConnectionState::Disconnected,
        tslib_core::ConnectionState::Connecting => TsLibConnectionState::Connecting,
        tslib_core::ConnectionState::Connected => TsLibConnectionState::Connected,
        tslib_core::ConnectionState::Initializing => TsLibConnectionState::Connected,
        tslib_core::ConnectionState::Reconnecting => TsLibConnectionState::Reconnecting,
    }
}

/// Send a message to the server
///
/// # Safety
/// - `client` must be a valid mutable pointer
/// - `message` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn tslib_client_send_server_message(
    client: *mut TsLibClient,
    message: *const c_char,
) -> TsLibError {
    if client.is_null() || message.is_null() {
        return TsLibError::InvalidArgument;
    }

    let client = &mut (*(client as *mut FfiClient)).client;
    let message = match CStr::from_ptr(message).to_str() {
        Ok(s) => s,
        Err(_) => return TsLibError::InvalidArgument,
    };

    match client.send_server_message(message) {
        Ok(()) => TsLibError::Ok,
        Err(_) => TsLibError::InternalError,
    }
}

/// Send a private message to a user
///
/// # Safety
/// - `client` must be a valid mutable pointer
/// - `message` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn tslib_client_send_private_message(
    client: *mut TsLibClient,
    user_id: u16,
    message: *const c_char,
) -> TsLibError {
    if client.is_null() || message.is_null() {
        return TsLibError::InvalidArgument;
    }

    let client = &mut (*(client as *mut FfiClient)).client;
    let message = match CStr::from_ptr(message).to_str() {
        Ok(s) => s,
        Err(_) => return TsLibError::InvalidArgument,
    };

    match client.send_private_message(user_id, message) {
        Ok(()) => TsLibError::Ok,
        Err(_) => TsLibError::InternalError,
    }
}

/// Move to a channel
///
/// # Safety
/// - `client` must be a valid mutable pointer
#[no_mangle]
pub unsafe extern "C" fn tslib_client_move_to_channel(
    client: *mut TsLibClient,
    channel_id: u64,
) -> TsLibError {
    if client.is_null() {
        return TsLibError::InvalidArgument;
    }

    let client = &mut (*(client as *mut FfiClient)).client;

    match client.move_to_channel(channel_id) {
        Ok(()) => TsLibError::Ok,
        Err(_) => TsLibError::ChannelError,
    }
}

/// Wait until the initial handshake and state sync are complete.
#[no_mangle]
pub unsafe extern "C" fn tslib_client_wait_connected(client: *mut TsLibClient) -> TsLibError {
    if client.is_null() {
        return TsLibError::InvalidArgument;
    }

    let handle = &mut *(client as *mut FfiClient);
    let FfiClient { client, runtime, .. } = handle;
    match runtime.block_on(client.wait_connected()) {
        Ok(()) => TsLibError::Ok,
        Err(_) => TsLibError::ConnectionFailed,
    }
}

/// Install a callback for text messages. The callback runs on the thread
/// calling `tslib_client_pump` and must not call back into the same client.
#[no_mangle]
pub unsafe extern "C" fn tslib_client_set_text_callback(
    client: *mut TsLibClient,
    callback: Option<TextMessageCallback>,
    user_data: *mut c_void,
) -> TsLibError {
    if client.is_null() {
        return TsLibError::InvalidArgument;
    }

    let handle = &mut *(client as *mut FfiClient);
    handle.text_callback = callback;
    handle.user_data = user_data;
    TsLibError::Ok
}

/// Install a callback for connection state changes.
#[no_mangle]
pub unsafe extern "C" fn tslib_client_set_state_callback(
    client: *mut TsLibClient,
    callback: Option<ConnectionStateCallback>,
    user_data: *mut c_void,
) -> TsLibError {
    if client.is_null() {
        return TsLibError::InvalidArgument;
    }

    let handle = &mut *(client as *mut FfiClient);
    handle.state_callback = callback;
    handle.user_data = user_data;
    TsLibError::Ok
}

/// Drive the connection and deliver queued events to the registered callbacks.
///
/// Returns the number of events delivered, or `-1` on error. This must be
/// called repeatedly (for example from the application's main loop) so the
/// connection can send/receive packets.
#[no_mangle]
pub unsafe extern "C" fn tslib_client_pump(
    client: *mut TsLibClient,
    timeout_ms: u32,
) -> c_int {
    if client.is_null() {
        return -1;
    }

    let handle = &mut *(client as *mut FfiClient);
    let FfiClient {
        client,
        runtime,
        text_callback,
        state_callback,
        user_data,
    } = handle;

    let events = match runtime.block_on(client.wait_events(timeout_ms as u64)) {
        Ok(events) => events,
        Err(_) => return -1,
    };

    let count = events.len() as c_int;
    for event in events {
        match event {
            Event::TextMessage {
                sender_id,
                sender_name,
                message,
                target,
                ..
            } => {
                if let Some(callback) = *text_callback {
                    let name = CString::new(sender_name).unwrap_or_default();
                    let message = CString::new(message).unwrap_or_default();
                    let is_private = matches!(
                        target,
                        tslib_core::events::MessageTarget::Private
                    ) as c_int;
                    callback(
                        sender_id,
                        name.as_ptr(),
                        message.as_ptr(),
                        is_private,
                        *user_data,
                    );
                }
            }
            Event::Connected { .. } | Event::Reconnected { .. } => {
                if let Some(callback) = *state_callback {
                    callback(
                        TsLibConnectionState::Connecting,
                        TsLibConnectionState::Connected,
                        *user_data,
                    );
                }
            }
            Event::Disconnected { .. } | Event::ConnectionLost { .. } => {
                if let Some(callback) = *state_callback {
                    callback(
                        TsLibConnectionState::Connected,
                        TsLibConnectionState::Disconnected,
                        *user_data,
                    );
                }
            }
            _ => {}
        }
    }

    count
}

/// Free a client
#[no_mangle]
pub unsafe extern "C" fn tslib_client_free(client: *mut TsLibClient) {
    if !client.is_null() {
        drop(Box::from_raw(client as *mut FfiClient));
    }
}

// ============================================================================
// Utility functions
// ============================================================================

/// Free a string returned by tslib functions
#[no_mangle]
pub unsafe extern "C" fn tslib_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// Get the library version
#[no_mangle]
pub extern "C" fn tslib_version() -> *const c_char {
    static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION.as_ptr() as *const c_char
}

/// Initialize the library (call once at startup)
#[no_mangle]
pub extern "C" fn tslib_init() -> TsLibError {
    // Initialize tracing
    tracing_subscriber::fmt::try_init().ok();

    TsLibError::Ok
}

/// Shutdown the library (call once at exit)
#[no_mangle]
pub extern "C" fn tslib_shutdown() {
    // Nothing to do for now
}
