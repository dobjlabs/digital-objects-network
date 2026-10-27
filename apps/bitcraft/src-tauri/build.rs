fn main() {
    println!("cargo:rerun-if-env-changed=BITCRAFT_DISCORD_APPLICATION_ID");
    println!("cargo:rerun-if-changed=../shell");
    println!(
        "cargo:rustc-env=BITCRAFT_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
    tauri_build::build();
}
