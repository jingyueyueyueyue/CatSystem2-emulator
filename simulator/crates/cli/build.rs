use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=windows.manifest");
    println!("cargo:rerun-if-env-changed=WINDRES");
    let target = env::var("TARGET").expect("Cargo TARGET");
    if !target.contains("windows") { return; }
    let source = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("crate directory"))
        .join("windows.manifest");
    if target.ends_with("msvc") {
        println!("cargo:rustc-link-arg-bin=cs2-player=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bin=cs2-player=/MANIFESTINPUT:{}", source.display());
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    // GNU ld needs a compiled manifest resource. Copy next to the resource
    // script so no quoted absolute source path crosses the C preprocessor.
    fs::copy(source, output.join("player.manifest")).expect("copy Windows manifest");
    fs::write(output.join("player.rc"), "1 24 \"player.manifest\"\n").expect("write manifest resource script");
    let compiler = env::var_os("WINDRES").unwrap_or_else(|| {
        let installed = PathBuf::from("C:/msys64/mingw64/bin/windres.exe");
        if installed.is_file() { installed.into_os_string() } else { "windres".into() }
    });
    let resource = output.join("player-resource.o");
    let status = Command::new(compiler).current_dir(&output)
        .args(["-i", "player.rc", "-O", "coff", "-o"])
        .arg(&resource).status().expect("run windres for Windows manifest");
    assert!(status.success(), "Windows manifest resource compilation failed");
    println!("cargo:rustc-link-arg-bin=cs2-player={}", resource.display());
}
