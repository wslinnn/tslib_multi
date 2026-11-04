//! Builds the vendored freedesktop webrtc-audio-processing 2.1 (WebRTC AEC3)
//! via meson + ninja and links it together with the C ABI wrapper in
//! `c/wrapper`.
//!
//! Targets:
//! - host (windows-msvc / linux / macos): native meson build. On MSVC the
//!   caller must provide a vcvars environment so `cl.exe` is on PATH.
//! - android (`aarch64-linux-android` / `x86_64-linux-android`): meson cross
//!   file against the NDK (`ANDROID_NDK_HOME`), API level 29, libc++_static.
//!
//! Tools required in PATH: meson (>= 0.63) and ninja. abseil is built as a
//! meson subproject from the committed `subprojects/packagecache/` (offline,
//! sha256-verified against `abseil-cpp.wrap`).
//!
//! Notes:
//! - `cpp_std` must be forced to c++20 on the meson command line: the
//!   upstream `default_options` say c++17, which MSVC rejects for the
//!   designated initializers in `input_volume_stats_reporter.cc`
//!   (error C7555); GCC/Clang only accept it as a C++17 extension.
//! - Archives are linked by name (`rustc-link-lib=static=...` plus
//!   `rustc-link-search`), the only primitives that propagate from a
//!   dependency build script to downstream links. MSVC archives are staged
//!   as `NAME.lib` because link.exe resolves nothing else; on ELF targets
//!   each abseil archive is declared twice so a single-pass linker resolves
//!   the inter-library cycles.
//! - The Android C++ runtime (`libc++.a`, a thin archive the rlib layer
//!   cannot ingest) is linked via `-C link-arg=-lc++` in
//!   `.cargo/config.toml`, effective at the final cdylib link only.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const WAP_SUBDIR: &str = "c/wap";
const WRAPPER_SUBDIR: &str = "c/wrapper";
const MAIN_LIB: &str = "libwebrtc-audio-processing-2.a";
const PRIVATEARCH_LIB: &str = "libwebrtc_audio_processing_privatearch.a";
const ABI_API_LEVEL: u32 = 29; // app minSdk

fn main() {
    println!("cargo:rerun-if-changed={WAP_SUBDIR}");
    println!("cargo:rerun-if-changed={WRAPPER_SUBDIR}");
    println!("cargo:rerun-if-env-changed=ANDROID_NDK_HOME");

    let target = env::var("TARGET").expect("TARGET unset");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR unset"));
    let wap_dir = manifest.join(WAP_SUBDIR);
    // One meson build tree per target triple: a host-configured tree holds
    // COFF/ELF objects for the wrong platform, and reconfiguring in place
    // would thrash between targets.
    let build_dir = wap_dir.join("build").join(&target);
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR unset"));

    let meson = require_tool("meson");
    let ninja = require_tool("ninja");

    configure(&meson, &wap_dir, &build_dir, &target, &out_dir);
    let mut ninja_cmd = Command::new(&ninja);
    ninja_cmd.current_dir(&build_dir);
    run(ninja_cmd, "ninja");

    // Compile the C ABI wrapper against the wap headers (source tree is
    // self-contained: -I c/wap/webrtc resolves the "api/..." includes; the
    // abseil sources are extracted into subprojects/ by meson setup).
    let mut cc_build = cc::Build::new();
    cc_build
        .cpp(true)
        .file(manifest.join(WRAPPER_SUBDIR).join("aec3_wrapper.cpp"))
        .include(wap_dir.join("webrtc"))
        .include(abseil_source_dir(&wap_dir))
        .std("c++20")
        .warnings(false);
    cc_build.compile("tslib_aec3_wrapper");

    let archives = collect_archives(&build_dir);
    let link_names = stage_for_linker(&out_dir, &archives, &target);
    emit_link_args(&out_dir, &build_dir, &link_names, &target);
}

/// NDK prebuilt directory tag for the HOST running this build (build
/// scripts execute on the host even when cross-compiling — derive from the
/// HOST triple, never from TARGET).
fn ndk_host_tag() -> &'static str {
    match env::var("HOST").as_deref() {
        Ok(h) if h.contains("windows") => "windows-x86_64",
        Ok(h) if h.contains("darwin") || h.contains("macos") => "darwin-x86_64",
        _ => "linux-x86_64",
    }
}

fn require_tool(name: &str) -> String {
    // Windows executables carry an extension (meson.exe shim, ninja.exe);
    // probe the common ones instead of bare `name`.
    let candidates: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.cmd"), format!("{name}.bat"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    for dir in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        for candidate in &candidates {
            let path = dir.join(candidate);
            if path.is_file() {
                return path.display().to_string();
            }
        }
    }
    panic!("`{name}` not found on PATH — required to build the vendored AEC3 (install meson and ninja, e.g. `scoop install meson` / `apt install meson ninja-build`)")
}

fn run(mut cmd: Command, what: &str) {
    let status = cmd.status().unwrap_or_else(|e| panic!("{what} failed to spawn: {e}"));
    if !status.success() {
        panic!("{what} exited with {status}");
    }
}

/// `meson setup` once; later build.rs runs reuse the configured build dir
/// (ninja handles incremental rebuilds from source timestamps).
fn configure(meson: &str, wap_dir: &Path, build_dir: &Path, target: &str, out_dir: &Path) {
    if build_dir.join("build.ninja").exists() {
        return;
    }
    let mut cmd = Command::new(meson);
    cmd.arg("setup")
        .arg("-Dbuildtype=release")
        .arg("-Ddefault_library=static")
        .arg("-Dcpp_std=c++20");
    if target.contains("android") {
        let cross_file = write_android_cross_file(out_dir, target);
        cmd.arg("--cross-file").arg(&cross_file);
    }
    cmd.arg(build_dir);
    cmd.current_dir(wap_dir);
    run(cmd, "meson setup");
}

/// Meson cross file for one Android ABI, targeting the NDK's unified clang
/// driver (versioned wrapper scripts differ per host OS; clang.exe/.clang
/// plus `--target` works everywhere).
fn write_android_cross_file(out_dir: &Path, target: &str) -> PathBuf {
    let ndk = PathBuf::from(
        env::var("ANDROID_NDK_HOME")
            .or_else(|_| env::var("ANDROID_NDK_ROOT"))
            .expect("ANDROID_NDK_HOME (or ANDROID_NDK_ROOT) must point at the NDK for Android builds"),
    );
    let host_tag = ndk_host_tag();
    let llvm = ndk.join("toolchains/llvm/prebuilt").join(host_tag).join("bin");

    let (cpu_family, cpu, triple) = if target.contains("aarch64") {
        ("aarch64", "aarch64", format!("aarch64-linux-android{ABI_API_LEVEL}"))
    } else if target.contains("x86_64") {
        ("x86_64", "x86_64", format!("x86_64-linux-android{ABI_API_LEVEL}"))
    } else {
        panic!("unsupported Android target: {target}");
    };

    let sysroot = ndk.join("toolchains/llvm/prebuilt").join(host_tag).join("sysroot");
    let target_args = format!("['--target={triple}', '--sysroot={}']", quote_meson(&sysroot));

    let file = out_dir.join(format!("android-{cpu}.ini"));
    std::fs::write(
        &file,
        format!(
            "[binaries]\n\
             c = '{c}'\n\
             cpp = '{cpp}'\n\
             ar = '{ar}'\n\
             strip = '{strip}'\n\
             \n\
             [built-in options]\n\
             c_args = {target_args}\n\
             cpp_args = {target_args}\n\
             c_link_args = {target_args}\n\
             cpp_link_args = {target_args}\n\
             \n\
             [host_machine]\n\
             system = 'android'\n\
             cpu_family = '{cpu_family}'\n\
             cpu = '{cpu}'\n\
             endian = 'little'\n\
             \n\
             [properties]\n\
             needs_exe_wrapper = true\n",
            c = quote_meson(&llvm.join("clang")),
            cpp = quote_meson(&llvm.join("clang++")),
            ar = quote_meson(&llvm.join("llvm-ar")),
            strip = quote_meson(&llvm.join("llvm-strip")),
        ),
    )
    .expect("write meson cross file");
    file
}

fn quote_meson(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// Meson extracts the abseil subproject source next to its `.wrap` file
/// (in the vendored source tree, not the build dir).
fn abseil_source_dir(wap_dir: &Path) -> PathBuf {
    wap_dir.join("subprojects").join("abseil-cpp-20240722.0")
}

/// Built abseil archives land in the build tree.
fn abseil_build_dir(build_dir: &Path) -> PathBuf {
    build_dir
        .join("subprojects")
        .join("abseil-cpp-20240722.0")
}

/// Main library first, then the x86 AVX2 private-arch addendum, then every
/// abseil archive (alphabetical — order only matters without a linker group).
fn collect_archives(build_dir: &Path) -> Vec<PathBuf> {
    let mut archives = vec![build_dir
        .join("webrtc/modules/audio_processing")
        .join(MAIN_LIB)];
    let privatearch = build_dir
        .join("webrtc/modules/audio_processing")
        .join(PRIVATEARCH_LIB);
    if privatearch.exists() {
        archives.push(privatearch);
    }
    let abseil_dir = abseil_build_dir(build_dir);
    let mut abseil: Vec<PathBuf> = std::fs::read_dir(&abseil_dir)
        .unwrap_or_else(|e| panic!("abseil build dir missing ({e}) — did meson build the subproject?"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("libabsl_") && n.ends_with(".a"))
                .unwrap_or(false)
        })
        .collect();
    abseil.sort();
    archives.extend(abseil);
    archives
}

/// Stage archives with conventional linker names. MSVC's link.exe resolves
/// `-lstatic=NAME` against `NAME.lib` only, while meson names MSVC static
/// archives `libNAME.a` — copy them to a staging dir renamed. Other
/// toolchains keep the meson names and search the original dirs.
fn stage_for_linker(out_dir: &Path, archives: &[PathBuf], target: &str) -> Vec<String> {
    let archive_name = |p: &Path| -> String {
        p.file_name()
            .and_then(|n| n.to_str())
            .expect("archive file name")
            .to_string()
    };
    if !target.contains("msvc") {
        return archives.iter().map(|p| archive_name(p.as_path())).collect();
    }
    let staging = out_dir.join("link");
    std::fs::create_dir_all(&staging).expect("create link staging dir");
    archives
        .iter()
        .map(|src| {
            let stem = archive_name(src)
                .trim_end_matches(".a")
                .trim_start_matches("lib")
                .to_string();
            let dst = staging.join(format!("{stem}.lib"));
            std::fs::copy(src, &dst).expect("stage archive for MSVC link");
            stem
        })
        .collect()
}

fn emit_link_args(out_dir: &Path, build_dir: &Path, archive_names: &[String], target: &str) {
    // Only `rustc-link-lib` / `rustc-link-search` propagate from a dependency
    // build script to downstream links (rustc-link-arg does not), so archives
    // are resolved by name in the staged dirs.
    if target.contains("msvc") {
        let staging = out_dir.join("link");
        println!("cargo:rustc-link-search=native={}", staging.display());
    } else {
        println!(
            "cargo:rustc-link-search=native={}",
            build_dir.join("webrtc/modules/audio_processing").display()
        );
        println!(
            "cargo:rustc-link-search=native={}",
            abseil_build_dir(build_dir).display()
        );
    }

    // Dependency order: wrapper objects (via cc) come first from rustc's own
    // args, then the APM library, then abseil. ELF linkers resolve archives
    // in a single left-to-right pass and abseil has inter-library cycles
    // (e.g. log -> strings), so each abseil archive is declared twice — the
    // second pass resolves whatever the first pass left open. MSVC resolves
    // archives in multiple passes and ignores the repetition.
    for name in archive_names.iter().take(2) {
        let lib = name.trim_end_matches(".a").trim_start_matches("lib");
        println!("cargo:rustc-link-lib=static={lib}");
    }
    let absl_count = archive_names.len().saturating_sub(2);
    for i in 0..absl_count {
        let name = &archive_names[2 + i];
        let lib = name.trim_end_matches(".a").trim_start_matches("lib");
        println!("cargo:rustc-link-lib=static={lib}");
    }
    for i in 0..absl_count {
        let name = &archive_names[2 + i];
        let lib = name.trim_end_matches(".a").trim_start_matches("lib");
        println!("cargo:rustc-link-lib=static={lib}");
    }

    if target.contains("windows") {
        // winmm: timeGetTime in rtc_base; MSVC pulls the CRT C++ runtime
        // automatically via the defaultlibs of the import archive.
        println!("cargo:rustc-link-lib=dylib=winmm");
    } else if target.contains("android") {
        // The static C++ runtime (libc++.a — a thin archive rustc's rlib
        // layer cannot ingest) is linked via `-C link-arg=-lc++` in
        // .cargo/config.toml; the NDK clang driver searches the per-API
        // sysroot dir itself, so nothing to emit here.
    } else {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
}
