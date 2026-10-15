use std::{env, fs, path::PathBuf};

fn main() {
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    fs::copy("../embassy-pico/memory.x", output.join("memory.x")).expect("Pico memory layout");
    println!("cargo:rustc-link-search={}", output.display());
    println!("cargo:rustc-link-arg=-Tlink.x");
    println!("cargo:rustc-link-arg=-Tlink-rp.x");
    println!("cargo:rustc-link-arg=-Tdefmt.x");
    println!("cargo:rerun-if-changed=../embassy-pico/memory.x");
}
