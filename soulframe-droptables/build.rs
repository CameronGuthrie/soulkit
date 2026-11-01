//! Build script: copy the shared workspace `oo2core_9.dll` next to the built
//! binary so it can be loaded at runtime without setting `SOULFRAME_OODLE_LIB`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=../lib/oo2core_9.dll");

    if !cfg!(windows) {
        return;
    }

    let dll_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("lib").join("oo2core_9.dll"));

    let dll_src = match dll_src {
        Some(p) if p.exists() => p,
        _ => {
            println!(
                "cargo:warning=oo2core_9.dll not found in workspace lib/. Set SOULFRAME_OODLE_LIB at runtime."
            );
            return;
        }
    };

    // OUT_DIR -> .../target/<profile>/build/<crate>/out  — climb 3 levels to <profile>/.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let profile_dir = out_dir.ancestors().nth(3).expect("OUT_DIR too shallow");

    let dst = profile_dir.join("oo2core_9.dll");
    if let Err(e) = fs::copy(&dll_src, &dst) {
        println!(
            "cargo:warning=Failed to copy Oodle DLL to {}: {}",
            dst.display(),
            e
        );
    }
    // Also copy into deps/ so example/test executables find it too.
    let deps_dst = profile_dir.join("deps").join("oo2core_9.dll");
    let _ = fs::copy(&dll_src, &deps_dst);
}
