// Minimal C ABI wrapper over the WebRTC AudioProcessing APM (AEC3) for
// tslib-audio-aec3. AEC3 works on 10 ms chunks: the Rust engine feeds one
// 960-sample (20 ms @ 48 kHz) frame as two process_render/process_capture
// pairs of sample_rate/100 samples.
//
// Engine selection: AEC3 only. The rest of the APM chain (NS, AGC1/2, HPF)
// stays off — noise suppression is owned by the app pipeline (RNNoise), and
// double processing degrades both stages.

#include "aec3_wrapper.h"

#include <algorithm>
#include <memory>
#include <vector>

#include "modules/audio_processing/include/audio_processing.h"

namespace {

// 2.x returns a ref-counted AudioProcessing (scoped_refptr); the Engine owns
// one reference and drops it on delete.
struct Engine {
    rtc::scoped_refptr<webrtc::AudioProcessing> apm;
    webrtc::StreamConfig config;
    std::vector<int16_t> render_src;
    std::vector<int16_t> render_dst;
};

}  // namespace

extern "C" void *tslib_aec3_create(int32_t sample_rate) {
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
    config.noise_suppression.enabled = false;
    config.high_pass_filter.enabled = false;
    engine->apm->ApplyConfig(config);

    engine->config = webrtc::StreamConfig(sample_rate, 1);
    const size_t frames = engine->config.num_frames();
    engine->render_src.assign(frames, 0);
    engine->render_dst.assign(frames, 0);
    return engine.release();
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
