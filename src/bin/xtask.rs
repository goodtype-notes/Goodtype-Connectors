//! `cargo xtask verify`: the one verification entry point.
use std::process::Command;

fn run(program: &str, args: &[&str]) {
    let status = Command::new(program)
        .args(args)
        .status()
        .unwrap_or_else(|error| panic!("cannot run {program}: {error}"));
    if !status.success() {
        std::process::exit(1);
    }
}

fn main() {
    assert_eq!(
        std::env::args().nth(1).as_deref(),
        Some("verify"),
        "usage: cargo xtask verify"
    );
    run("cargo", &["fmt", "--all", "--", "--check"]);
    run("cargo", &["test", "--locked"]);
    run(
        "cargo",
        &[
            "clippy",
            "--locked",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    );
    run(
        "python",
        &[
            "-m",
            "unittest",
            "discover",
            "-s",
            "scripts",
            "-p",
            "test_*.py",
        ],
    );
}
