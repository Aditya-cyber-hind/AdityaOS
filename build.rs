use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let obj = out_dir.join("boot.o");

    let status = Command::new("aarch64-linux-gnu-gcc")
        .args(&[
            "-c",
            "-o", obj.to_str().unwrap(),
            "src/boot.S",
        ])
        .status()
        .expect("failed to run aarch64-linux-gnu-gcc");

    assert!(status.success(), "assembling boot.S failed");

    // Tell cargo to link boot.o
    println!("cargo:rustc-link-arg={}", obj.display());

    // Rebuild if boot.S changes
    println!("cargo:rerun-if-changed=src/boot.S");
    println!("cargo:rerun-if-changed=linker.ld");
}
