#ifndef __SPEEX_TYPES_H__
#define __SPEEX_TYPES_H__

/*
 * Fixed replacement for the autotools-generated speexdsp_config_types.h.
 * The upstream template probes sizes because speexdsp targets exotic DSPs;
 * every platform this crate builds for (Android arm64/x86_64, Windows x64,
 * Linux x64) has the standard fixed-width types, so the probe is skipped.
 * Same declarations as the template would produce there.
 */

#include <stdint.h>

typedef int16_t spx_int16_t;
typedef uint16_t spx_uint16_t;
typedef int32_t spx_int32_t;
typedef uint32_t spx_uint32_t;

#endif
