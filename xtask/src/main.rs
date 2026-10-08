//! `cargo xtask ci` — format check + workspace tests.

fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "help".into());
    match cmd.as_str() {
        "ci" => {
            run("cargo", &["fmt", "--all", "--", "--check"]);
            run("cargo", &["test", "--workspace", "--offline"]);
        }
        "test" => run("cargo", &["test", "--workspace"]),
        "fmt" => run("cargo", &["fmt", "--all"]),
        "windows" => {
            let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/package-windows.sh");
            let st = std::process::Command::new("bash")
                .arg(&script)
                .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
                .status()
                .unwrap_or_else(|e| panic!("package-windows.sh: {e}"));
            if !st.success() {
                std::process::exit(st.code().unwrap_or(1));
            }
        }
        _ => {
            eprintln!("xtask ci | test | fmt | windows");
        }
    }
}

fn run(bin: &str, args: &[&str]) {
    let st = std::process::Command::new(bin).args(args).status().unwrap_or_else(|e| panic!("{bin}: {e}"));
    if !st.success() {
        std::process::exit(st.code().unwrap_or(1));
    }
}
