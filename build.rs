fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os == "emscripten" {
        println!("cargo:rustc-link-arg=-sJSPI");
        println!("cargo:rustc-link-arg=-sINITIAL_HEAP=536870912");
    }
}
