// Included from `apps/encodecraft/build.rs` and `apps/encodecraft-cli/build.rs`.
// Embeds `assets/app-icon/encodecraft.ico` when that file exists (Windows target only).

fn main() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    let ico = manifest.join("../../assets/app-icon/encodecraft.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    println!("cargo:rerun-if-changed={}", manifest.join("../../assets/app-icon").display());

    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if !windows || !ico.is_file() {
        return;
    }
    let Some(path) = ico.to_str() else {
        eprintln!("app icon path is not UTF-8: {}", ico.display());
        std::process::exit(1);
    };
    let ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut res = winresource::WindowsResource::new();
    res.set_icon(path);
    res.set("ProductName", "EncodeCraft");
    res.set("FileDescription", "EncodeCraft");
    res.set("ProductVersion", &ver);
    res.set("FileVersion", &ver);
    res.set("LegalCopyright", "EncodeCraft contributors");
    if let Err(err) = res.compile() {
        eprintln!("could not embed Windows app icon {path}: {err}");
        std::process::exit(1);
    }
}
