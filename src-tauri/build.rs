fn main() {
    println!("cargo:rerun-if-env-changed=STATIC_VCRUNTIME");
    tauri_build::build()
}
