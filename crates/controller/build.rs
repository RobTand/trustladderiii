fn main() {
    println!("cargo::rustc-check-cfg=cfg(loom)");
    if std::env::var_os("CARGO_FEATURE_LOOM").is_some() {
        println!("cargo::rustc-cfg=loom");
    }
}
