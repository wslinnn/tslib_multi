//! Builds the vendored speexdsp echo-cancellation subset (`speexdsp/`).

fn main() {
    let mut build = cc::Build::new();
    build
        .include("speexdsp/include")
        .include("speexdsp/libspeexdsp")
        // Float processing path: modern phones and hosts have fast FPUs, and
        // the fixed-point path saturates audibly at 48 kHz frame sizes.
        .define("FLOATING_POINT", None)
        // fftwrap dispatch: use the self-contained smallft (no kiss_fft needed).
        .define("USE_SMALLFT", None)
        // Normally provided by autotools' generated config.h (symbol
        // visibility prefix); empty is correct for both MSVC and static
        // linking on Unix.
        .define("EXPORT", "")
        .file("speexdsp/libspeexdsp/mdf.c")
        .file("speexdsp/libspeexdsp/fftwrap.c")
        .file("speexdsp/libspeexdsp/smallft.c")
        .warnings(false);
    build.compile("speexdsp_echo");

    for file in [
        "speexdsp/libspeexdsp/mdf.c",
        "speexdsp/libspeexdsp/fftwrap.c",
        "speexdsp/libspeexdsp/fftwrap.h",
        "speexdsp/libspeexdsp/smallft.c",
        "speexdsp/libspeexdsp/smallft.h",
        "speexdsp/libspeexdsp/arch.h",
        "speexdsp/libspeexdsp/math_approx.h",
        "speexdsp/libspeexdsp/os_support.h",
        "speexdsp/libspeexdsp/pseudofloat.h",
        "speexdsp/libspeexdsp/fixed_generic.h",
        "speexdsp/libspeexdsp/fixed_arm4.h",
        "speexdsp/libspeexdsp/fixed_arm5e.h",
        "speexdsp/libspeexdsp/fixed_bfin.h",
        "speexdsp/libspeexdsp/fixed_debug.h",
        "speexdsp/include/speex/speex_echo.h",
        "speexdsp/include/speex/speexdsp_types.h",
    ] {
        println!("cargo:rerun-if-changed={}", file);
    }
}
