use jni::objects::{JClass, JString};
use jni::sys::{jint, jlong, jstring};
use jni::JNIEnv;

use crate::error::to_jni_result;
use crate::require_string;

fn ptr_to_identity(ptr: jlong) -> &'static mut tslib_core::Identity {
    unsafe { &mut *(ptr as *mut tslib_core::Identity) }
}

fn identity_to_ptr(id: tslib_core::Identity) -> jlong {
    Box::into_raw(Box::new(id)) as jlong
}

/// `Identity()` — create a new random identity.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeCreate(
    mut env: JNIEnv,
    _class: JClass,
) -> jlong {
    match to_jni_result(&mut env, tslib_core::Identity::create()) {
        Some(id) => identity_to_ptr(id),
        None => 0,
    }
}

/// `Identity.nativeDestroy(ptr)` — free memory.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeDestroy(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) {
    if ptr != 0 {
        unsafe {
            drop(Box::from_raw(ptr as *mut tslib_core::Identity));
        }
    }
}

/// `Identity.load(path)` — load from file.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeLoad(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) -> jlong {
    let path = match require_string(&mut env, &path) {
        Ok(s) => s,
        Err(()) => return 0,
    };
    match to_jni_result(&mut env, tslib_core::Identity::load(&path)) {
        Some(id) => identity_to_ptr(id),
        None => 0,
    }
}

/// `Identity.save(path)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeSave(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    path: JString,
) {
    let path = match require_string(&mut env, &path) {
        Ok(s) => s,
        Err(()) => return,
    };
    let id = ptr_to_identity(ptr);
    to_jni_result(&mut env, id.save(&path));
}

/// `Identity.fromString(data)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeFromString(
    mut env: JNIEnv,
    _class: JClass,
    data: JString,
) -> jlong {
    let data = match require_string(&mut env, &data) {
        Ok(s) => s,
        Err(()) => return 0,
    };
    match to_jni_result(&mut env, tslib_core::Identity::from_string(&data)) {
        Some(id) => identity_to_ptr(id),
        None => 0,
    }
}

/// `Identity.exportString()`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeExportString(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) -> jstring {
    let id = ptr_to_identity(ptr);
    match to_jni_result(&mut env, id.export_string()) {
        Some(s) => env
            .new_string(&s)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

/// `Identity.exportTeamSpeakIni(label)` — official client ts.ini export.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeExportTeamSpeakIni(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    label: JString,
) -> jstring {
    let label = match require_string(&mut env, &label) {
        Ok(s) => s,
        Err(()) => return std::ptr::null_mut(),
    };
    let id = ptr_to_identity(ptr);
    match to_jni_result(&mut env, id.export_team_speak_ini(&label)) {
        Some(s) => env
            .new_string(&s)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

/// `Identity.getUniqueId()`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeGetUniqueId(
    env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) -> jstring {
    let id = ptr_to_identity(ptr);
    let uid = id.unique_id();
    env.new_string(&uid)
        .map(|js| js.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// `Identity.getSecurityLevel()`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeGetSecurityLevel(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) -> jint {
    let id = ptr_to_identity(ptr);
    id.security_level() as jint
}

/// `Identity.getNickname()`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeGetNickname(
    env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) -> jstring {
    let id = ptr_to_identity(ptr);
    match id.nickname() {
        Some(n) => env
            .new_string(n)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

/// `Identity.setNickname(name)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeSetNickname(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    name: JString,
) {
    let name = match require_string(&mut env, &name) {
        Ok(s) => s,
        Err(()) => return,
    };
    let id = ptr_to_identity(ptr);
    id.set_nickname(name);
}

/// `Identity.improve(targetLevel)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_Identity_nativeImprove(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    target_level: jint,
) {
    let id = ptr_to_identity(ptr);
    to_jni_result(&mut env, id.improve(target_level as u8));
}
