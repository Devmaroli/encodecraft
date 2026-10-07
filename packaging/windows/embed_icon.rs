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
    let path = ico.to_str().expect("app icon path must be UTF-8");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(path);
    res.set("ProductName", "EncodeCraft");
    res.set("FileDescription", "EncodeCraft");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed {path}: {e}");
    }
}
