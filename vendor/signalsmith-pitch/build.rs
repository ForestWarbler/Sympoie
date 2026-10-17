fn main() {
    println!("cargo:rerun-if-changed=native/bridge.cpp");
    println!("cargo:rerun-if-changed=upstream");
    cc::Build::new()
        .cpp(true)
        .std("c++14")
        .opt_level(3)
        .warnings(false)
        .include("upstream/stretch")
        .include("upstream/linear/include")
        .file("native/bridge.cpp")
        .compile("sympoie_signalsmith_pitch");
}
