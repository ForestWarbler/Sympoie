fn main() {
    println!("cargo:rerun-if-changed=src/bridge.cpp");
    println!("cargo:rerun-if-changed=upstream");
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++11")
        .opt_level(3)
        .define("RUBBERBAND_STATIC", None)
        .file("upstream/single/RubberBandSingle.cpp")
        .file("src/bridge.cpp")
        .warnings(false);
    build.compile("sympoie_rubberband_pitch");
    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        println!("cargo:rustc-link-lib=framework=Accelerate");
    }
}
