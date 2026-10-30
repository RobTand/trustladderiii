fn main() {
    println!("cargo::rustc-check-cfg=cfg(loom)");
    println!("cargo::rerun-if-env-changed=TL3_LOOM");
    if std::env::var("TL3_LOOM").is_ok_and(|value| value == "1") {
        println!("cargo::rustc-cfg=loom");
    }
}
