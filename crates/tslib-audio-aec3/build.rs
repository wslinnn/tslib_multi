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
//! - Static archives are passed to the linker as explicit full paths (not
//!   `-lstatic` names): meson names MSVC archives `*.a`, which link.exe
//!   would not resolve by search path, and full paths also let us wrap the
//!   abseil set in `--start-group`/`--end-group` on ELF targets without
//!   guessing a topological order.

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
    let build_dir = wap_dir.join("build");
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
    let link_paths = stage_for_linker(&out_dir, &archives, &target);
    emit_link_args(&link_paths, &target);
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
    let host_tag = match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => "windows-x86_64",
        Ok("macos") => "darwin-x86_64",
        _ => "linux-x86_64",
    };
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

/// MSVC's link.exe resolves archives by `NAME.lib` only, while meson names
/// MSVC static archives `libNAME.a` — copy them to a staging dir with
/// conventional names. Other toolchains link the original paths directly.
fn stage_for_linker(out_dir: &Path, archives: &[PathBuf], target: &str) -> Vec<PathBuf> {
    if !target.contains("msvc") {
        return archives.to_vec();
    }
    let staging = out_dir.join("link");
    std::fs::create_dir_all(&staging).expect("create link staging dir");
    archives
        .iter()
        .map(|src| {
            let stem = src
                .file_name()
                .and_then(|n| n.to_str())
                .expect("archive file name")
                .trim_end_matches(".a")
                .trim_start_matches("lib")
                .to_string();
            let dst = staging.join(format!("{stem}.lib"));
            std::fs::copy(src, &dst).expect("stage archive for MSVC link");
            dst
        })
        .collect()
}

fn emit_link_args(link_paths: &[PathBuf], target: &str) {
    // Wrapper objects (via cc) come first from rustc's own library args; the
    // archives are appended after them, which is the order static archives
    // need. ELF linkers resolve archives in one pass, so the abseil set is
    // wrapped in a group to avoid topological-order failures.
    let grouped = target.contains("linux") || target.contains("android");
    if grouped {
        println!("cargo:rustc-link-arg=-Wl,--start-group");
    }
    for path in link_paths {
        println!("cargo:rustc-link-arg={}", path.display());
    }
    if grouped {
        println!("cargo:rustc-link-arg=-Wl,--end-group");
    }

    if target.contains("windows") {
        // winmm: timeGetTime in rtc_base; MSVC pulls the CRT C++ runtime
        // automatically via the defaultlibs of the import archive.
        println!("cargo:rustc-link-lib=dylib=winmm");
    } else if target.contains("android") {
        // libc++_static/libc++abi live in the NDK sysroot per-API dir.
        let ndk = PathBuf::from(
            env::var("ANDROID_NDK_HOME")
                .or_else(|_| env::var("ANDROID_NDK_ROOT"))
                .expect("ANDROID_NDK_HOME unset"),
        );
        let host_tag = match env::var("CARGO_CFG_TARGET_OS").as_deref() {
            Ok("windows") => "windows-x86_64",
            Ok("macos") => "darwin-x86_64",
            _ => "linux-x86_64",
        };
        let triple = if target.contains("aarch64") {
            "aarch64-linux-android"
        } else {
            "x86_64-linux-android"
        };
        let sysroot_lib = ndk
            .join("toolchains/llvm/prebuilt")
            .join(host_tag)
            .join("sysroot/usr/lib")
            .join(triple)
            .join(ABI_API_LEVEL.to_string());
        println!("cargo:rustc-link-search=native={}", sysroot_lib.display());
        println!("cargo:rustc-link-lib=static=c++_static");
        println!("cargo:rustc-link-lib=static=c++abi");
    } else {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
}
