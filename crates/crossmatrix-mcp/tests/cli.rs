//! End-to-end CLI acceptance tests: run the real binary and observe its output
//! and exit status through the public command-line interface.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crossmatrix-mcp"))
        .args(args)
        .output()
        .expect("run crossmatrix-mcp")
}

fn dir_str(dir: &Path) -> String {
    dir.to_string_lossy().into_owned()
}

fn initialized_store() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    run(&["init", "--dir", &dir_str(tmp.path())]);
    let root = tmp.path().join(".crossmatrix");
    (tmp, root)
}

#[test]
fn version_long_prints_package_version_and_exits_zero() {
    let out = run(&["--version"]);
    assert_eq!(
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned()
        ),
        (Some(0), "crossmatrix-mcp 0.2.1\n".to_string())
    );
}

#[test]
fn version_short_prints_package_version_and_exits_zero() {
    let out = run(&["-V"]);
    assert_eq!(
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned()
        ),
        (Some(0), "crossmatrix-mcp 0.2.1\n".to_string())
    );
}

#[test]
fn help_exits_zero() {
    assert_eq!(run(&["--help"]).status.code(), Some(0));
}

#[test]
fn init_dir_creates_the_readme() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = run(&["init", "--dir", &dir_str(tmp.path())]);
    assert_eq!(
        (
            out.status.code(),
            tmp.path().join(".crossmatrix/README.md").is_file()
        ),
        (Some(0), true)
    );
}

#[test]
fn check_dir_on_a_fresh_init_exits_zero() {
    let (_tmp, root) = initialized_store();
    let out = run(&["check", "--dir", &dir_str(root.parent().unwrap())]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn check_dir_reports_a_broken_state_file() {
    let (_tmp, root) = initialized_store();
    std::fs::write(root.join("state/broken.json"), "{ not valid json").expect("write broken state");
    let out = run(&["check", "--dir", &dir_str(root.parent().unwrap())]);
    assert_eq!(
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).contains("state/broken.json")
        ),
        (Some(1), true)
    );
}

#[test]
fn unknown_command_exits_two() {
    assert_eq!(run(&["frobnicate"]).status.code(), Some(2));
}
