use std::path::PathBuf;

fn copy_runtime_dlls() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("lib");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let profile = out.ancestors().nth(3).expect("Cargo profile directory");
    for name in ["oo2core_9.dll", "libzstd.dll", "vcruntime140.dll"] {
        let input = source.join(name);
        println!("cargo:rerun-if-changed={}", input.display());
        if !input.is_file() {
            println!(
                "cargo:warning=missing runtime DLL {}; extraction needs it beside soulkit.exe",
                input.display()
            );
            continue;
        }
        for folder in [profile.to_path_buf(), profile.join("deps")] {
            std::fs::create_dir_all(&folder).expect("create runtime DLL directory");
            let destination = folder.join(name);
            // Other workspace tools copy the same DLLs. Avoid rewriting a file
            // currently loaded by an open app when its contents already match.
            let bytes = std::fs::read(&input).expect("read runtime DLL");
            if std::fs::read(&destination).ok().as_deref() != Some(bytes.as_slice()) {
                std::fs::write(&destination, &bytes).unwrap_or_else(|error| {
                    panic!(
                        "Cannot copy {} to {}: {error}",
                        input.display(),
                        destination.display()
                    )
                });
            }
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities");
    println!("cargo:rerun-if-changed=icons");
    copy_runtime_dlls();
    tauri_build::build();
}
