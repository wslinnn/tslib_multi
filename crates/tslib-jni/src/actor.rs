//! JNI bindings for the actor-backed Java `ActorClient`.

use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jint, jlong, jobjectArray};
use jni::JNIEnv;
use std::time::Duration;

use tslib_core::{ActorEventStream, ActorHandle, ClientConfig, ConnectionState, Error};

use crate::error::throw_tslib_exception;
use crate::types::create_java_event;
use crate::{get_string, require_string};

struct JniActor {
    actor: ActorHandle,
    events: ActorEventStream,
    runtime: tokio::runtime::Runtime,
}

fn actor_ptr(ptr: jlong) -> &'static mut JniActor {
    unsafe { &mut *(ptr as *mut JniActor) }
}

fn error_code(error: Error) -> jint {
    use tslib_core::error::ConnectionError;
    match error {
        Error::InvalidArgument(_) => 1,
        Error::Connection(ConnectionError::NotConnected) => 3,
        Error::Timeout => 4,
        Error::PermissionDenied { .. } => 5,
        Error::Channel(_) => 6,
        Error::Identity(_) => 7,
        Error::Connection(_) => 2,
        _ => 99,
    }
}

fn state_code(state: ConnectionState) -> jint {
    match state {
        ConnectionState::Disconnected => 0,
        ConnectionState::Connecting | ConnectionState::Initializing => 1,
        ConnectionState::Connected => 2,
        ConnectionState::Reconnecting => 3,
    }
}

/// `ActorClient.nativeCreate(address, identityPtr, nickname, password)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_ActorClient_nativeCreate(
    mut env: JNIEnv,
    _class: JClass,
    address: JString,
    identity_ptr: jlong,
    nickname: JString,
    password: JString,
) -> jlong {
    let address = match require_string(&mut env, &address) {
        Ok(value) => value,
        Err(()) => return 0,
    };
    let nickname = match require_string(&mut env, &nickname) {
        Ok(value) => value,
        Err(()) => return 0,
    };
    if identity_ptr == 0 {
        throw_tslib_exception(&mut env, "Identity pointer is null");
        return 0;
    }
    let identity = unsafe { &*(identity_ptr as *const tslib_core::Identity) }.clone();
    let password = get_string(&mut env, &password);

    let mut builder = ClientConfig::builder()
        .address(address)
        .identity(identity)
        .nickname(nickname);
    if let Some(password) = password {
        builder = builder.password(password);
    }
    let config = match builder.build() {
        Ok(config) => config,
        Err(error) => {
            throw_tslib_exception(&mut env, &format!("Invalid config: {error}"));
            return 0;
        }
    };

    let actor = match ActorHandle::spawn(config) {
        Ok(actor) => actor,
        Err(error) => {
            throw_tslib_exception(&mut env, &format!("Actor connect failed: {error}"));
            return 0;
        }
    };
    let events = actor.subscribe();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            throw_tslib_exception(&mut env, &format!("Runtime failed: {error}"));
            return 0;
        }
    };

    Box::into_raw(Box::new(JniActor {
        actor,
        events,
        runtime,
    })) as jlong
}

/// `ActorClient.nativePump(ptr, timeoutMs)` — returns `Event[]`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_ActorClient_nativePump(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    timeout_ms: jint,
) -> jobjectArray {
    if ptr == 0 {
        return std::ptr::null_mut();
    }
    let handle = actor_ptr(ptr);
    let timeout = Duration::from_millis(timeout_ms.max(0) as u64);

    let first = handle.runtime.block_on(handle.events.recv_timeout(timeout));
    let mut events = Vec::new();
    if let Some(event) = first {
        events.push(event);
    }
    while let Some(event) = handle.events.try_recv() {
        events.push(event);
    }

    let event_class = match env.find_class("dev/tslib/Event") {
        Ok(class) => class,
        Err(_) => return std::ptr::null_mut(),
    };
    let array = match env.new_object_array(events.len() as i32, &event_class, &JObject::null()) {
        Ok(array) => array,
        Err(_) => return std::ptr::null_mut(),
    };
    for (index, event) in events.iter().enumerate() {
        let object = create_java_event(&mut env, event);
        let _ = env.set_object_array_element(&array, index as i32, &object);
    }
    array.into_raw()
}

/// `ActorClient.nativeSendServerMessage(ptr, message, timeoutMs)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_ActorClient_nativeSendServerMessage(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    message: JString,
    timeout_ms: jint,
) -> jint {
    if ptr == 0 {
        return 1;
    }
    let message = match require_string(&mut env, &message) {
        Ok(value) => value,
        Err(()) => return 1,
    };
    let handle = actor_ptr(ptr);
    let result = handle.runtime.block_on(handle.actor.execute(
        Duration::from_millis(timeout_ms.max(1) as u64),
        move |client| client.send_server_message(message),
    ));
    match result {
        Ok(()) => 0,
        Err(error) => error_code(error),
    }
}

/// `ActorClient.nativeSendPrivateMessage(ptr, userId, message, timeoutMs)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_ActorClient_nativeSendPrivateMessage(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    user_id: jint,
    message: JString,
    timeout_ms: jint,
) -> jint {
    if ptr == 0 {
        return 1;
    }
    let message = match require_string(&mut env, &message) {
        Ok(value) => value,
        Err(()) => return 1,
    };
    let handle = actor_ptr(ptr);
    let result = handle.runtime.block_on(handle.actor.execute(
        Duration::from_millis(timeout_ms.max(1) as u64),
        move |client| client.send_private_message(user_id as u16, message),
    ));
    match result {
        Ok(()) => 0,
        Err(error) => error_code(error),
    }
}

/// `ActorClient.nativeMoveToChannel(ptr, channelId, timeoutMs)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_ActorClient_nativeMoveToChannel(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    channel_id: jlong,
    timeout_ms: jint,
) -> jint {
    if ptr == 0 {
        return 1;
    }
    let handle = actor_ptr(ptr);
    let result = handle.runtime.block_on(handle.actor.execute(
        Duration::from_millis(timeout_ms.max(1) as u64),
        move |client| client.move_to_channel(channel_id as u64),
    ));
    match result {
        Ok(()) => 0,
        Err(error) => error_code(error),
    }
}

/// `ActorClient.nativeSetInputMuted(ptr, muted, timeoutMs)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_ActorClient_nativeSetInputMuted(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    muted: jboolean,
    timeout_ms: jint,
) -> jint {
    if ptr == 0 {
        return 1;
    }
    let muted = muted != 0;
    let handle = actor_ptr(ptr);
    let result = handle.runtime.block_on(handle.actor.execute(
        Duration::from_millis(timeout_ms.max(1) as u64),
        move |client| client.set_input_muted(muted),
    ));
    match result {
        Ok(()) => 0,
        Err(error) => error_code(error),
    }
}

/// `ActorClient.nativeState(ptr)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_ActorClient_nativeState(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) -> jint {
    if ptr == 0 {
        return 0;
    }
    state_code(actor_ptr(ptr).actor.state())
}

/// `ActorClient.nativeDisconnect(ptr)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_ActorClient_nativeDisconnect(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    let handle = actor_ptr(ptr);
    let _ = handle.runtime.block_on(handle.actor.clone().shutdown());
}

/// `ActorClient.nativeFree(ptr)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_ActorClient_nativeFree(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) {
    if ptr != 0 {
        unsafe {
            drop(Box::from_raw(ptr as *mut JniActor));
        }
    }
}
