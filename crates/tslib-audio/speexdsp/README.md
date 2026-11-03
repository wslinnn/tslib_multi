# Vendored speexdsp subset (echo cancellation)

Upstream: https://github.com/xiph/speexdsp tag `SpeexDSP-1.2.1`
(commit `1b28a0f61bc31162979e1f26f3981fc3637095c8`).

License: BSD (revised), see `COPYING`. All original file headers are kept
intact. BSD permits redistribution inside this crate (MIT OR Apache-2.0).

Only the files needed by the software echo canceller (`src/echo.rs`, the
`mdf.c` multichannel frequency-domain adaptive filter) are vendored:

- `libspeexdsp/mdf.c` — echo canceller core
- `libspeexdsp/fftwrap.c` (+`.h`) — FFT dispatch (`USE_SMALLFT`)
- `libspeexdsp/smallft.c` (+`.h`) — float FFT used by the dispatch
- `libspeexdsp/arch.h`, `math_approx.h`, `os_support.h`,
  `pseudofloat.h`, `fixed_generic.h`, `fixed_arm4.h`, `fixed_arm5e.h`,
  `fixed_bfin.h`, `fixed_debug.h` — internal headers of the two sources
- `include/speex/speex_echo.h`, `include/speex/speexdsp_types.h` — public API

Build configuration (see `build.rs`): `-DFLOATING_POINT -DUSE_SMALLFT`.
No upstream code is modified; configuration is done purely through defines.

One generated file is replaced by a fixed stand-in:
`include/speex/speexdsp_config_types.h` (upstream generates it from
`speexdsp_config_types.h.in` at configure time; all platforms this crate
targets use the standard fixed-width types — see the file header).
