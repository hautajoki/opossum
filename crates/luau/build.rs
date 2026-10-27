//! Builds Luau's compiler (luau0-src) and the shim that keeps C++ exceptions on the C++ side (`src/shim.cpp`),
//! for the O2 gate (`src/gate.rs`).

fn main() {
    println!("cargo:rerun-if-changed=src/shim.cpp");
    cc::Build::new().cpp(true).std("c++17").warnings(true).file("src/shim.cpp").compile("opossum_luau_shim");
    luau0_src::Build::new().build().print_cargo_metadata();
}
