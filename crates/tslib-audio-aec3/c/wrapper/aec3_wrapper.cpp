// Minimal C ABI wrapper over the WebRTC AudioProcessing APM (AEC3) for
// tslib-audio-aec3. AEC3 works on 10 ms chunks: the Rust engine feeds one
// 960-sample (20 ms @ 48 kHz) frame as two process_render/process_capture
// pairs of sample_rate/100 samples.
//
// Engine selection: AEC3 plus a static +6 dB post-gain on the capture
// output. The raw VOICE_PERFORMANCE capture path skips every vendor gain
// stage and arrives uniformly quiet; the post-gain lifts the transmitted
// level back into a healthy range. It is applied by the APM after all
// capture processing (capture_level_adjustment.post_gain_factor), so AEC3
// keeps operating on native-level input: a pre-gain slot was measured to
// lift the noise floor into the AEC3 suppressor's comfort zone and cost
// real suppression. AGC2 was also evaluated for this slot and rejected: it
// sits after AEC, scores speech-shaped residual echo as speech, and
// re-amplifies what AEC3 just removed (measured ERLE collapse in echo-only
// segments).
//
// Noise suppression runs inside the APM; the capture path has no vendor noise
// suppression at all (VOICE_PERFORMANCE). AGC1/2 stay off — the fixed post-gain
// replaces AGC.
//
// High-pass filtering is NOT off, despite what `config.high_pass_filter` says:
// AEC3 forces it on for every full-band, non-mobile session
// (`enforce_high_pass_filtering` defaults to true in the APM config, and
// `InitializeHighPassFilter` builds the filter for AEC regardless of
// `high_pass_filter.enabled`). The APM applies it to the capture buffer before
// the echo canceller, so the rumble/DC rejection the "enable HPF" suggestion
// asks for is already in the chain. Setting the flag would only add a second,
// redundant filter stage; leaving it false keeps one HPF in the path.
//
// NS defaults to HIGH because the raw capture path carries the full noise
// floor into AEC. It is now selectable per session (see
// `tslib_aec3_create_with_config`) so a device where HIGH dulls weak
// consonants can be dialled back without a rebuild; HIGH stays the default so
// existing callers keep today's behaviour exactly.

#include "aec3_wrapper.h"

#include <algorithm>
#include <memory>
#include <vector>

#include "modules/audio_processing/include/audio_processing.h"

namespace {

// Static post-gain on the capture output: +6 dB (x2). Chosen conservatively
// so loud speech on already-hot devices keeps >20 dB of headroom to full
// scale; raise only with device measurements in hand.
constexpr float kTxPostGainFactor = 2.0f;

// 2.x returns a ref-counted AudioProcessing (scoped_refptr); the Engine owns
// one reference and drops it on delete.
struct Engine {
    rtc::scoped_refptr<webrtc::AudioProcessing> apm;
    webrtc::StreamConfig config;
    std::vector<int16_t> render_src;
    std::vector<int16_t> render_dst;
};

// Map the wrapper's NS level (0=low 1=moderate 2=high 3=very high) onto the
// APM enum. Out-of-range values fall back to HIGH rather than silently
// disabling suppression.
webrtc::AudioProcessing::Config::NoiseSuppression::Level NsLevelFromInt(
    int32_t level) {
    using Ns = webrtc::AudioProcessing::Config::NoiseSuppression;
    switch (level) {
        case 0:
            return Ns::kLow;
        case 1:
            return Ns::kModerate;
        case 3:
            return Ns::kVeryHigh;
        case 2:
        default:
            return Ns::kHigh;
    }
}

}  // namespace

extern "C" float tslib_aec3_post_gain(void) {
    return kTxPostGainFactor;
}

extern "C" void *tslib_aec3_create_with_config(int32_t sample_rate,
                                               int32_t ns_level) {
    if (sample_rate != 16000 && sample_rate != 32000 && sample_rate != 48000) {
        return nullptr;
    }
    std::unique_ptr<Engine> engine(new Engine());
    engine->apm = webrtc::AudioProcessingBuilder().Create();

    webrtc::AudioProcessing::Config config;
    config.echo_canceller.enabled = true;
    config.echo_canceller.mobile_mode = false;
    config.gain_controller1.enabled = false;
    config.gain_controller2.enabled = false;
    config.capture_level_adjustment.enabled = true;
    config.capture_level_adjustment.pre_gain_factor = 1.0f;
    config.capture_level_adjustment.post_gain_factor = kTxPostGainFactor;
    config.capture_level_adjustment.analog_mic_gain_emulation.enabled = false;
    config.noise_suppression.enabled = true;
    config.noise_suppression.level = NsLevelFromInt(ns_level);
    // Left off on purpose: AEC3 already builds and applies its own HPF for
    // full-band sessions (see the header comment). Enabling this would add a
    // second stage on top of it.
    config.high_pass_filter.enabled = false;
    engine->apm->ApplyConfig(config);

    engine->config = webrtc::StreamConfig(sample_rate, 1);
    const size_t frames = engine->config.num_frames();
    engine->render_src.assign(frames, 0);
    engine->render_dst.assign(frames, 0);
    return engine.release();
}

extern "C" void *tslib_aec3_create(int32_t sample_rate) {
    // ns_level 2 == kHigh: the historical behaviour, byte for byte.
    return tslib_aec3_create_with_config(sample_rate, 2);
}

extern "C" int32_t tslib_aec3_process_render(void *handle,
                                             const int16_t *far_ref,
                                             int32_t samples) {
    if (handle == nullptr) {
        return -1;
    }
    auto *engine = static_cast<Engine *>(handle);
    if (samples <= 0 ||
        static_cast<size_t>(samples) != engine->config.num_frames()) {
        return -2;
    }
    std::copy(far_ref, far_ref + samples, engine->render_src.begin());
    int status = engine->apm->ProcessReverseStream(
        engine->render_src.data(), engine->config, engine->config,
        engine->render_dst.data());
    return status;
}

extern "C" int32_t tslib_aec3_process_capture(void *handle, const int16_t *mic,
                                              int16_t *out, int32_t samples) {
    if (handle == nullptr) {
        return -1;
    }
    auto *engine = static_cast<Engine *>(handle);
    if (samples <= 0 ||
        static_cast<size_t>(samples) != engine->config.num_frames()) {
        return -2;
    }
    return engine->apm->ProcessStream(mic, engine->config, engine->config, out);
}

extern "C" void tslib_aec3_destroy(void *handle) {
    delete static_cast<Engine *>(handle);
}
