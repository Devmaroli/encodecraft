// Optional window icon: compile in `assets/app-icon/encodecraft-256.png` (or `-64.png`) if present.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    let dir = manifest.join("../../assets/app-icon");
    println!("cargo:rerun-if-changed={}", dir.display());
    for name in ["encodecraft-256.png", "encodecraft-64.png"] {
        println!("cargo:rerun-if-changed={}", dir.join(name).display());
    }

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let dest = out.join("app_icon.rs");

    let png = ["encodecraft-256.png", "encodecraft-64.png"].into_iter().map(|n| dir.join(n)).find(|p| p.is_file());

    let src = if let Some(png) = png {
        let path = png.to_str().expect("icon path UTF-8").replace('\\', "/");
        format!(
            "pub fn png_bytes() -> Option<&'static [u8]> {{ Some(include_bytes!({path:?})) }}\n"
        )
    } else {
        "pub fn png_bytes() -> Option<&'static [u8]> { None }\n".into()
    };
    fs::write(dest, src).expect("write app_icon.rs");
}
