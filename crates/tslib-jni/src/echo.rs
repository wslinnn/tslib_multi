//! `dev.tslib.EchoCanceller` JNI surface — software echo cancellation.
//!
//! One handle owns one [`Aec3EchoEngine`]. The engine is stateful and must
//! be called from a single thread (the app calls it from the capture loop);
//! `process` reads `mic`/`far`, writes `out`, and returns the sample count.
//! Errors throw `TsLibException`; a failed frame must be bypassed by the
//! caller (forward `mic` unchanged).

use jni::objects::{JClass, JShortArray};
use jni::sys::{jint, jlong};
use jni::JNIEnv;

use tslib_audio::echo::EchoEngine;
use tslib_audio_aec3::Aec3EchoEngine;

use crate::error::{audio_to_jni_result, throw_tslib_exception};

struct EchoHandle {
    engine: Aec3EchoEngine,
    frame_samples: usize,
}

fn ptr_to_echo(ptr: jlong) -> Option<&'static mut EchoHandle> {
    if ptr == 0 {
        None
    } else {
        Some(unsafe { &mut *(ptr as *mut EchoHandle) })
    }
}

/// `EchoCanceller(sampleRate, frameSamples, tailSamples)` — create.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_EchoCanceller_nativeCreate(
    mut env: JNIEnv,
    _class: JClass,
    sample_rate: jint,
    frame_samples: jint,
    tail_samples: jint,
) -> jlong {
    if sample_rate <= 0 || frame_samples <= 0 || tail_samples <= 0 {
        throw_tslib_exception(
            &mut env,
            &format!(
                "EchoCanceller requires positive sizes (sample_rate={sample_rate}, frame_samples={frame_samples}, tail_samples={tail_samples})"
            ),
        );
        return 0;
    }
    let engine = match audio_to_jni_result(
        &mut env,
        Aec3EchoEngine::new(
            sample_rate as u32,
            frame_samples as usize,
            tail_samples as usize,
        ),
    ) {
        Some(e) => e,
        None => return 0,
    };
    let handle = EchoHandle {
        engine,
        frame_samples: frame_samples as usize,
    };
    Box::into_raw(Box::new(handle)) as jlong
}

/// `EchoCanceller.nativeDestroy(ptr)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_EchoCanceller_nativeDestroy(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) {
    if ptr != 0 {
        unsafe {
            drop(Box::from_raw(ptr as *mut EchoHandle));
        }
    }
}

/// `EchoCanceller.reset()` — discard the learned path (route change).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_EchoCanceller_nativeReset(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) {
    let Some(handle) = ptr_to_echo(ptr) else {
        throw_tslib_exception(&mut env, "EchoCanceller handle is null");
        return;
    };
    if audio_to_jni_result(&mut env, handle.engine.reset()).is_none() {
        // Exception already thrown by the helper.
    }
}

/// `EchoCanceller.nativeZeroedFrames(ptr)` — cumulative count of frames the
/// blow-up guard zeroed. A zeroed frame is a 20 ms hole in the transmitted
/// stream; the app diffs this per diagnostics window to surface any non-zero
/// rate in its logs.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_EchoCanceller_nativeZeroedFrames(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) -> jlong {
    match ptr_to_echo(ptr) {
        Some(handle) => handle.engine.zeroed_frames() as jlong,
        None => -1,
    }
}

/// `EchoCanceller.process(mic, far, out)` — cancel one frame.
///
/// Returns the number of samples written to `out` (the frame size), or a
/// negative value after throwing.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_tslib_EchoCanceller_nativeProcess(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    mic: JShortArray,
    far: JShortArray,
    out: JShortArray,
) -> jint {
    let Some(handle) = ptr_to_echo(ptr) else {
        throw_tslib_exception(&mut env, "EchoCanceller handle is null");
        return -1;
    };
    let n = handle.frame_samples;

    for (name, array) in [("mic", &mic), ("far", &far), ("out", &out)] {
        let len = match env.get_array_length(array) {
            Ok(l) => l as usize,
            Err(e) => {
                throw_tslib_exception(&mut env, &format!("Failed to read {name} length: {e}"));
                return -1;
            }
        };
        if len != n {
            throw_tslib_exception(
                &mut env,
                &format!("EchoCanceller {name} array length {len} != frame size {n}"),
            );
            return -1;
        }
    }

    let mut mic_buf = vec![0i16; n];
    let mut far_buf = vec![0i16; n];
    if let Err(e) = env.get_short_array_region(&mic, 0, &mut mic_buf) {
        throw_tslib_exception(&mut env, &format!("Failed to read mic array: {e}"));
        return -1;
    }
    if let Err(e) = env.get_short_array_region(&far, 0, &mut far_buf) {
        throw_tslib_exception(&mut env, &format!("Failed to read far array: {e}"));
        return -1;
    }

    let mut out_buf = vec![0i16; n];
    let written = match audio_to_jni_result(
        &mut env,
        handle.engine.process(&mic_buf, &far_buf, &mut out_buf),
    ) {
        Some(w) => w,
        None => return -1,
    };

    if let Err(e) = env.set_short_array_region(&out, 0, &out_buf) {
        throw_tslib_exception(&mut env, &format!("Failed to write out array: {e}"));
        return -1;
    }
    written as jint
}
