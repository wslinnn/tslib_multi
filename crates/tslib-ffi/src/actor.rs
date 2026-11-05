//! Actor-backed C API.
//!
//! The legacy `tslib_client_*` functions keep owning `Client` directly for
//! compatibility. The `tslib_actor_*` functions wrap [`tslib_core::ActorHandle`]:
//! commands block with a timeout and report the real result, while events are
//! drained by `tslib_actor_pump` without blocking the connection.

use std::ffi::{c_int, c_void, CStr, CString};
use std::ptr;

use tslib_core::{ActorEventStream, ActorHandle, ClientConfig, Error, Event, Identity};

use crate::{
    map_connection_state, ConnectionStateCallback, TextMessageCallback, TsLibActor,
    TsLibConnectionState, TsLibError, TsLibIdentity,
};

struct FfiActor {
    actor: ActorHandle,
    events: ActorEventStream,
    runtime: tokio::runtime::Runtime,
    text_callback: Option<(TextMessageCallback, *mut c_void)>,
    state_callback: Option<(ConnectionStateCallback, *mut c_void)>,
    last_state: TsLibConnectionState,
}

fn map_actor_error(error: Error) -> TsLibError {
    use tslib_core::error::ConnectionError;
    match error {
        Error::Timeout => TsLibError::Timeout,
        Error::InvalidArgument(_) => TsLibError::InvalidArgument,
        Error::PermissionDenied { .. } => TsLibError::PermissionDenied,
        Error::Channel(_) => TsLibError::ChannelError,
        Error::Identity(_) => TsLibError::IdentityError,
        Error::Connection(ConnectionError::NotConnected) => TsLibError::NotConnected,
        Error::Connection(_) => TsLibError::ConnectionFailed,
        _ => TsLibError::InternalError,
    }
}

/// Connect through the actor. Blocks until the initial state is synchronized.
///
/// # Safety
/// Same string/identity requirements as `tslib_client_connect`. The returned
/// pointer must be freed with `tslib_actor_free`.
#[no_mangle]
pub unsafe extern "C" fn tslib_actor_connect(
    address: *const std::ffi::c_char,
    identity: *const TsLibIdentity,
    nickname: *const std::ffi::c_char,
    password: *const std::ffi::c_char,
) -> *mut TsLibActor {
    if address.is_null() || identity.is_null() || nickname.is_null() {
        return ptr::null_mut();
    }
    let Ok(address) = CStr::from_ptr(address).to_str() else {
        return ptr::null_mut();
    };
    let Ok(nickname) = CStr::from_ptr(nickname).to_str() else {
        return ptr::null_mut();
    };
    let identity = (*(identity as *const Identity)).clone();
    let password = if password.is_null() {
        None
    } else {
        CStr::from_ptr(password).to_str().ok()
    };

    let mut builder = ClientConfig::builder()
        .address(address)
        .identity(identity)
        .nickname(nickname);
    if let Some(password) = password {
        builder = builder.password(password);
    }
    let Ok(config) = builder.build() else {
        return ptr::null_mut();
    };

    let Ok(actor) = ActorHandle::spawn(config) else {
        return ptr::null_mut();
    };
    let state = actor.state();
    let events = actor.subscribe();
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return ptr::null_mut();
    };

    Box::into_raw(Box::new(FfiActor {
        actor,
        events,
        runtime,
        text_callback: None,
        state_callback: None,
        last_state: map_connection_state(state),
    })) as *mut TsLibActor
}

unsafe fn ffi_actor<'a>(actor: *mut TsLibActor) -> Option<&'a mut FfiActor> {
    if actor.is_null() {
        None
    } else {
        Some(&mut *(actor as *mut FfiActor))
    }
}

/// Register a text-message callback for an actor client.
#[no_mangle]
pub unsafe extern "C" fn tslib_actor_set_text_callback(
    actor: *mut TsLibActor,
    callback: Option<TextMessageCallback>,
    user_data: *mut c_void,
) -> TsLibError {
    let Some(actor) = ffi_actor(actor) else {
        return TsLibError::InvalidArgument;
    };
    actor.text_callback = callback.map(|callback| (callback, user_data));
    TsLibError::Ok
}

/// Register a connection-state callback for an actor client.
#[no_mangle]
pub unsafe extern "C" fn tslib_actor_set_state_callback(
    actor: *mut TsLibActor,
    callback: Option<ConnectionStateCallback>,
    user_data: *mut c_void,
) -> TsLibError {
    let Some(actor) = ffi_actor(actor) else {
        return TsLibError::InvalidArgument;
    };
    actor.state_callback = callback.map(|callback| (callback, user_data));
    TsLibError::Ok
}

unsafe fn dispatch_actor_event(actor: &mut FfiActor, event: &Event) {
    if let Event::TextMessage {
        sender_id,
        sender_name,
        message,
        target,
        ..
    } = event
    {
        if let Some((callback, user_data)) = actor.text_callback {
            let name = CString::new(sender_name.as_str()).unwrap_or_default();
            let message = CString::new(message.as_str()).unwrap_or_default();
            let is_private =
                matches!(target, tslib_core::events::MessageTarget::Private) as c_int;
            callback(
                *sender_id,
                name.as_ptr(),
                message.as_ptr(),
                is_private,
                user_data,
            );
        }
    }
}

/// Drain actor events (waiting up to `timeout_ms` for the first one).
///
/// Returns the number of events drained, or `-1` on invalid input.
#[no_mangle]
pub unsafe extern "C" fn tslib_actor_pump(actor: *mut TsLibActor, timeout_ms: u32) -> c_int {
    let Some(actor) = ffi_actor(actor) else {
        return -1;
    };

    let first = actor
        .runtime
        .block_on(
            actor
                .events
                .recv_timeout(std::time::Duration::from_millis(timeout_ms as u64)),
        );
    let mut count = 0;
    if let Some(event) = first {
        dispatch_actor_event(actor, &event);
        count += 1;
    }
    while let Some(event) = actor.events.try_recv() {
        dispatch_actor_event(actor, &event);
        count += 1;
    }

    let state = map_connection_state(actor.actor.state());
    if state != actor.last_state {
        if let Some((callback, user_data)) = actor.state_callback {
            callback(actor.last_state, state, user_data);
        }
        actor.last_state = state;
    }

    count
}

/// Actor client state.
#[no_mangle]
pub unsafe extern "C" fn tslib_actor_state(actor: *mut TsLibActor) -> TsLibConnectionState {
    match ffi_actor(actor) {
        Some(actor) => map_connection_state(actor.actor.state()),
        None => TsLibConnectionState::Disconnected,
    }
}

/// Voice events dropped for this actor because its consumer was too slow.
#[no_mangle]
pub unsafe extern "C" fn tslib_actor_dropped_voice(actor: *mut TsLibActor) -> u64 {
    match ffi_actor(actor) {
        Some(actor) => actor.events.dropped_voice_events(),
        None => 0,
    }
}

/// Send a server message through the actor and wait for the result.
#[no_mangle]
pub unsafe extern "C" fn tslib_actor_send_server_message(
    actor: *mut TsLibActor,
    message: *const std::ffi::c_char,
    timeout_ms: u32,
) -> TsLibError {
    let Some(actor) = ffi_actor(actor) else {
        return TsLibError::InvalidArgument;
    };
    if message.is_null() {
        return TsLibError::InvalidArgument;
    }
    let Ok(message) = CStr::from_ptr(message).to_str() else {
        return TsLibError::InvalidArgument;
    };
    let message = message.to_string();
    let result = actor.runtime.block_on(actor.actor.execute(
        std::time::Duration::from_millis(timeout_ms.max(1) as u64),
        move |client| client.send_server_message(message),
    ));
    match result {
        Ok(()) => TsLibError::Ok,
        Err(error) => map_actor_error(error),
    }
}

/// Send a private message through the actor and wait for the result.
#[no_mangle]
pub unsafe extern "C" fn tslib_actor_send_private_message(
    actor: *mut TsLibActor,
    user_id: u16,
    message: *const std::ffi::c_char,
    timeout_ms: u32,
) -> TsLibError {
    let Some(actor) = ffi_actor(actor) else {
        return TsLibError::InvalidArgument;
    };
    if message.is_null() {
        return TsLibError::InvalidArgument;
    }
    let Ok(message) = CStr::from_ptr(message).to_str() else {
        return TsLibError::InvalidArgument;
    };
    let message = message.to_string();
    let result = actor.runtime.block_on(actor.actor.execute(
        std::time::Duration::from_millis(timeout_ms.max(1) as u64),
        move |client| client.send_private_message(user_id, message),
    ));
    match result {
        Ok(()) => TsLibError::Ok,
        Err(error) => map_actor_error(error),
    }
}

/// Move to a channel through the actor and wait for the result.
#[no_mangle]
pub unsafe extern "C" fn tslib_actor_move_to_channel(
    actor: *mut TsLibActor,
    channel_id: u64,
    timeout_ms: u32,
) -> TsLibError {
    let Some(actor) = ffi_actor(actor) else {
        return TsLibError::InvalidArgument;
    };
    let result = actor.runtime.block_on(actor.actor.execute(
        std::time::Duration::from_millis(timeout_ms.max(1) as u64),
        move |client| client.move_to_channel(channel_id),
    ));
    match result {
        Ok(()) => TsLibError::Ok,
        Err(error) => map_actor_error(error),
    }
}

/// Disconnect and stop the actor.
#[no_mangle]
pub unsafe extern "C" fn tslib_actor_disconnect(actor: *mut TsLibActor) -> TsLibError {
    let Some(actor) = ffi_actor(actor) else {
        return TsLibError::InvalidArgument;
    };
    match actor.runtime.block_on(actor.actor.clone().shutdown()) {
        Ok(()) => TsLibError::Ok,
        Err(error) => map_actor_error(error),
    }
}

/// Free an actor handle.
#[no_mangle]
pub unsafe extern "C" fn tslib_actor_free(actor: *mut TsLibActor) {
    if !actor.is_null() {
        drop(Box::from_raw(actor as *mut FfiActor));
    }
}
