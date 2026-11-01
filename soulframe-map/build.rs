//! Copy `lib/oo2core_9.dll` from the workspace shared `lib/` dir into the
//! cargo target profile dir so `soulframe-map.exe` finds Oodle next to itself.

use std::path::PathBuf;

fn main() {
    if !cfg!(windows) {
        return;
    }

    println!("cargo:rerun-if-changed=../lib/oo2core_9.dll");

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = manifest_dir
        .parent()
        .expect("workspace root")
        .join("lib")
        .join("oo2core_9.dll");
    if !src.exists() {
        println!("cargo:warning=missing {}", src.display());
        return;
    }

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("profile dir ancestor")
        .to_path_buf();
    let deps_dir = profile_dir.join("deps");

    for dst in [
        profile_dir.join("oo2core_9.dll"),
        deps_dir.join("oo2core_9.dll"),
    ] {
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::copy(&src, &dst);
    }
}
