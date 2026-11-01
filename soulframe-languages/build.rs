//! Build script: copy `lib/oo2core_9.dll` and `lib/libzstd.dll` from the
//! workspace shared `lib/` dir into the cargo target profile dir so the built
//! exe and tests find them next to themselves.

use std::path::PathBuf;

fn main() {
    if !cfg!(windows) {
        return;
    }

    println!("cargo:rerun-if-changed=../lib/oo2core_9.dll");
    println!("cargo:rerun-if-changed=../lib/libzstd.dll");

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_dir = manifest_dir.parent().expect("workspace root").join("lib");

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    // OUT_DIR is .../target/<profile>/build/<crate>-<hash>/out
    // Climb 3 to reach .../target/<profile>/
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("profile dir ancestor")
        .to_path_buf();
    let deps_dir = profile_dir.join("deps");

    for dll in ["oo2core_9.dll", "libzstd.dll"] {
        let src = src_dir.join(dll);
        if !src.exists() {
            println!("cargo:warning=missing {}", src.display());
            continue;
        }
        for dst in [profile_dir.join(dll), deps_dir.join(dll)] {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&src, &dst);
        }
    }
}
