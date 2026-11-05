//! Links dav1d, VideoLAN's AV1 decoder, as a static library.
//!
//! - `MASKO_DAV1D_LIB_DIR` names the folder holding it: `dav1d.lib` on
//!   Windows (scripts/build-dav1d-windows.sh makes it and prints the
//!   folder), `libdav1d.a` elsewhere.
//! - Without it, on a Mac, the macOS library of apple/dav1d.xcframework
//!   (scripts/build-dav1d-apple.sh), so the tests run there.
//!
//! When there is no library (a `cargo check` for another target, say), the
//! build goes on with a warning and only linking fails.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MASKO_DAV1D_LIB_DIR");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let file = if target_env == "msvc" {
        "dav1d.lib"
    } else {
        "libdav1d.a"
    };

    let dir = match env::var_os("MASKO_DAV1D_LIB_DIR") {
        Some(dir) => Some(PathBuf::from(dir)),
        None if target_os == "macos" => {
            let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
            let root = manifest.parent().unwrap_or(&manifest).to_path_buf();
            Some(root.join("apple/dav1d.xcframework/macos-arm64_x86_64"))
        }
        None => None,
    };
    let Some(dir) = dir.filter(|dir| dir.join(file).is_file()) else {
        println!(
            "cargo:warning=no {file} for this target: set MASKO_DAV1D_LIB_DIR \
             (scripts/build-dav1d-windows.sh on Windows, scripts/build-dav1d-apple.sh on a Mac); \
             linking will fail without it"
        );
        return;
    };
    // The folder as given: canonical Windows paths (\\?\C:\...) can trip
    // up the MSVC linker.
    println!("cargo:rerun-if-changed={}", dir.join(file).display());
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static=dav1d");
}
