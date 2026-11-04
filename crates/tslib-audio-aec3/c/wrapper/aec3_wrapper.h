#ifndef TSLIB_AEC3_WRAPPER_H
#define TSLIB_AEC3_WRAPPER_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Create one AEC3 canceller for `sample_rate` (16000/32000/48000 only).
 * Returns an opaque handle, or NULL on failure. One instance per capture
 * session; all calls must come from a single thread. */
void *tslib_aec3_create(int32_t sample_rate);

/* Feed the 10 ms playback chunk the speaker is emitting now (reference for
 * the delay estimator). `samples` must equal sample_rate/100. Returns 0 on
 * success, a negative/-APM-error value otherwise. */
int32_t tslib_aec3_process_render(void *handle, const int16_t *far_ref,
                                  int32_t samples);

/* Cancel the echo of the fed render stream contained in `mic`, writing
 * `out` (may alias neither). `samples` must equal sample_rate/100. Returns
 * 0 on success, a negative/-APM-error value otherwise. */
int32_t tslib_aec3_process_capture(void *handle, const int16_t *mic,
                                   int16_t *out, int32_t samples);

void tslib_aec3_destroy(void *handle);

/* Fixed post-gain the engine applies to the capture output after AEC
 * (unitless factor, e.g. 2.0 for +6 dB). Callers use it to normalize engine
 * output when measuring echo suppression. */
float tslib_aec3_post_gain(void);

#ifdef __cplusplus
}
#endif

#endif /* TSLIB_AEC3_WRAPPER_H */
