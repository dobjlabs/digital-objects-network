fn main() {
    println!(
        "cargo:rustc-env=BITCRAFT_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
    tauri_build::build();
}
