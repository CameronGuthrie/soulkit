use std::path::PathBuf;

fn main() {
    if !cfg!(windows) {
        return;
    }
    println!("cargo:rerun-if-changed=../lib/oo2core_9.dll");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = root
        .parent()
        .expect("workspace root")
        .join("lib/oo2core_9.dll");
    if !source.exists() {
        return;
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let profile = out.ancestors().nth(3).expect("profile directory");
    for target in [
        profile.join("oo2core_9.dll"),
        profile.join("deps/oo2core_9.dll"),
    ] {
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::copy(&source, target);
    }
}
