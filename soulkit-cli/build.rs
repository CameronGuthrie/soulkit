//! Copy shared DLLs (Oodle, ZSTD) from `../lib/` into the cargo target
//! profile dir so `soulkit-cli.exe` finds them next to itself at runtime.

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
