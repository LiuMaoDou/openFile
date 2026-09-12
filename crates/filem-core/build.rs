fn main() {
    println!("cargo:rerun-if-changed=vendor/everything");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        cc::Build::new()
            .file("vendor/everything/src/Everything.c")
            .warnings(false)
            .compile("filem_everything");
        println!("cargo:rustc-link-lib=user32");
        println!("cargo:rustc-link-lib=shell32");
    }
}
