//! Builds Luau (vendor/luau0-src, once per output directory thanks to its build stamp) and our C++ over it: the shim
//! that keeps C++ exceptions on the C++ side of the O2 gate (`src/shim.cpp`, for `src/gate.rs`) and the VM
//! (`src/vm.cpp`, for `src/vm.rs`). Our C++ is compiled with Luau's own include directories and defines, so it sees
//! the same declarations and layouts the libraries were built with.

fn main() {
    println!("cargo:rerun-if-changed=src/shim.cpp");
    println!("cargo:rerun-if-changed=src/vm.cpp");
    let luau = luau0_src::Build::new().build();
    let mut shim = cc::Build::new();
    shim.cpp(true).std("c++17").warnings(true).includes(luau.include_dirs());
    for (name, value) in luau.defines() {
        shim.define(name, value.as_str());
    }
    shim.file("src/shim.cpp").file("src/vm.cpp").compile("opossum_luau_shim");
    luau.print_cargo_metadata();
}
