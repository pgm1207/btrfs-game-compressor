//! The bundled Intel texture encoder used for BC7 output is C++ and needs the
//! C++ runtime at link time. Only Linux is supported by this package.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-lib=stdc++");
    }
    // The library target exposes the Godot 3 transforms without the texture
    // codecs; nothing extra is needed there, but linking stdc++ is harmless.
}
