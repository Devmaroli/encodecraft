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
        _ => {
            eprintln!("xtask ci | test | fmt");
        }
    }
}

fn run(bin: &str, args: &[&str]) {
    let st = std::process::Command::new(bin).args(args).status().unwrap_or_else(|e| panic!("{bin}: {e}"));
    if !st.success() {
        std::process::exit(st.code().unwrap_or(1));
    }
}
