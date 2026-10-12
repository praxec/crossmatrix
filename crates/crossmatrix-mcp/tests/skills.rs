//! End-to-end acceptance tests for `crossmatrix-mcp skills install|uninstall|list`.
//!
//! Each test runs the real binary in throwaway project/home directories and
//! observes one public outcome: files on disk, stdout, or the exit status.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_crossmatrix-mcp");

struct Sandbox {
    home: tempfile::TempDir,
    project: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        Sandbox {
            home: tempfile::tempdir().expect("home"),
            project: tempfile::tempdir().expect("project"),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(args)
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            .output()
            .expect("run crossmatrix-mcp")
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn project(&self) -> &Path {
        self.project.path()
    }

    fn project_str(&self) -> String {
        self.project.path().to_string_lossy().into_owned()
    }

    fn install(&self, target: &str) -> Output {
        self.run(&[
            "skills",
            "install",
            "--target",
            target,
            "--project",
            &self.project_str(),
        ])
    }

    fn uninstall(&self, target: &str) -> Output {
        self.run(&[
            "skills",
            "uninstall",
            "--target",
            target,
            "--project",
            &self.project_str(),
        ])
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn walk(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn embedded_content(rel: &str) -> String {
    crossmatrix_mcp::skills::embedded_skills()
        .iter()
        .find(|(path, _)| *path == rel)
        .map(|(_, content)| (*content).to_string())
        .expect("embedded skill")
}

// ---------------------------------------------------------------- install

#[test]
fn install_claude_project_writes_the_main_skill_file() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    assert!(
        sandbox
            .project()
            .join(".claude/skills/crossmatrix/SKILL.md")
            .is_file()
    );
}

#[test]
fn install_claude_project_writes_every_embedded_skill_file() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    let mut files = Vec::new();
    walk(&sandbox.project().join(".claude/skills"), &mut files);
    assert_eq!(
        files
            .iter()
            .filter(|path| path.ends_with("SKILL.md"))
            .count(),
        8
    );
}

#[test]
fn install_records_a_sha256_for_each_written_file_in_the_manifest() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    let manifest_path = sandbox
        .project()
        .join(".claude/skills/.crossmatrix-skills.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("manifest"))
            .expect("valid manifest json");
    let recorded = manifest["files"].as_object().expect("files object");
    let root = sandbox.project().join(".claude/skills");
    assert!(
        crossmatrix_mcp::skills::embedded_skills()
            .iter()
            .all(|(rel, _)| {
                let expected = crossmatrix_mcp::skills::sha256_hex(
                    &std::fs::read(root.join(rel)).expect("file"),
                );
                recorded.get(*rel).and_then(|value| value.as_str()) == Some(expected.as_str())
            })
    );
}

#[test]
fn second_install_reports_every_file_unchanged() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    let out = sandbox.install("claude");
    assert!(stdout(&out).contains(
        "0 created, 0 updated, 8 unchanged, 0 modified (skipped), 0 foreign (skipped), 0 removed"
    ));
}

#[test]
fn a_user_edited_file_is_reported_modified_and_skipped() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    let edited = sandbox
        .project()
        .join(".claude/skills/crossmatrix-hoq/SKILL.md");
    std::fs::write(&edited, "user edit").expect("edit");
    let out = sandbox.install("claude");
    assert!(stdout(&out).contains("modified, skipped"));
}

#[test]
fn a_user_edited_file_is_left_untouched() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    let edited = sandbox
        .project()
        .join(".claude/skills/crossmatrix-hoq/SKILL.md");
    std::fs::write(&edited, "user edit").expect("edit");
    sandbox.install("claude");
    assert_eq!(std::fs::read_to_string(&edited).expect("read"), "user edit");
}

#[test]
fn force_overwrites_a_user_edited_file_with_the_shipped_content() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    let edited = sandbox
        .project()
        .join(".claude/skills/crossmatrix-hoq/SKILL.md");
    std::fs::write(&edited, "user edit").expect("edit");
    sandbox.run(&[
        "skills",
        "install",
        "--target",
        "claude",
        "--project",
        &sandbox.project_str(),
        "--force",
    ]);
    assert_eq!(
        std::fs::read_to_string(&edited).expect("read"),
        embedded_content("crossmatrix-hoq/SKILL.md")
    );
}

#[test]
fn a_preexisting_unknown_file_is_reported_foreign_and_skipped() {
    let sandbox = Sandbox::new();
    let foreign = sandbox
        .project()
        .join(".claude/skills/crossmatrix/SKILL.md");
    std::fs::create_dir_all(foreign.parent().expect("parent")).expect("create dir");
    std::fs::write(&foreign, "not ours").expect("write");
    let out = sandbox.install("claude");
    assert!(stdout(&out).contains("foreign, skipped"));
}

#[test]
fn dry_run_writes_nothing_under_the_project() {
    let sandbox = Sandbox::new();
    sandbox.run(&[
        "skills",
        "install",
        "--target",
        "claude",
        "--project",
        &sandbox.project_str(),
        "--dry-run",
    ]);
    assert!(!sandbox.project().join(".claude").exists());
}

#[test]
fn install_output_contains_the_claude_project_registration_command() {
    let sandbox = Sandbox::new();
    let out = sandbox.install("claude");
    assert!(stdout(&out).contains(
        "claude mcp add crossmatrix --scope project -- npx -y @matthew-cochran/crossmatrix"
    ));
}

#[test]
fn install_creates_no_mcp_configuration_anywhere_under_the_project() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    let mut files = Vec::new();
    walk(sandbox.project(), &mut files);
    let forbidden = files.iter().any(|path| {
        let rel = path.strip_prefix(sandbox.project()).expect("under project");
        let config_dir = rel.components().any(|c| {
            matches!(
                c.as_os_str().to_str(),
                Some(".cursor" | ".vscode" | ".gemini" | ".codex")
            )
        });
        let config_file = matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("mcp.json" | ".mcp.json" | "settings.json" | "config.toml")
        );
        config_dir || config_file
    });
    assert!(!forbidden);
}

#[test]
fn gemini_target_writes_the_hoq_command_file() {
    let sandbox = Sandbox::new();
    sandbox.install("gemini");
    assert!(
        sandbox
            .project()
            .join(".gemini/commands/crossmatrix-hoq.toml")
            .is_file()
    );
}

// ---------------------------------------------------------------- agents.md

#[test]
fn agents_md_target_writes_the_managed_block() {
    let sandbox = Sandbox::new();
    sandbox.install("agents-md");
    let text = std::fs::read_to_string(sandbox.project().join("AGENTS.md")).expect("AGENTS.md");
    assert!(
        text.contains(crossmatrix_mcp::skills::BLOCK_BEGIN)
            && text.contains(crossmatrix_mcp::skills::BLOCK_END)
    );
}

#[test]
fn second_agents_md_install_leaves_the_file_byte_identical() {
    let sandbox = Sandbox::new();
    sandbox.install("agents-md");
    let before = std::fs::read(sandbox.project().join("AGENTS.md")).expect("before");
    sandbox.install("agents-md");
    assert_eq!(
        std::fs::read(sandbox.project().join("AGENTS.md")).expect("after"),
        before
    );
}

#[test]
fn agents_md_with_user_scope_is_a_usage_error() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["skills", "install", "--target", "agents-md", "--user"]);
    assert_eq!(out.status.code(), Some(2));
}

// ---------------------------------------------------------------- uninstall

#[test]
fn uninstall_removes_an_unmodified_skill_file() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    sandbox.uninstall("claude");
    assert!(
        !sandbox
            .project()
            .join(".claude/skills/crossmatrix/SKILL.md")
            .exists()
    );
}

#[test]
fn uninstall_removes_the_manifest() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    sandbox.uninstall("claude");
    assert!(
        !sandbox
            .project()
            .join(".claude/skills/.crossmatrix-skills.json")
            .exists()
    );
}

#[test]
fn uninstall_keeps_a_user_edited_file() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    let edited = sandbox
        .project()
        .join(".claude/skills/crossmatrix-hoq/SKILL.md");
    std::fs::write(&edited, "user edit").expect("edit");
    sandbox.uninstall("claude");
    assert_eq!(std::fs::read_to_string(&edited).expect("read"), "user edit");
}

// ---------------------------------------------------------------- list

#[test]
fn list_reports_the_installed_root() {
    let sandbox = Sandbox::new();
    sandbox.install("claude");
    let out = sandbox.run(&["skills", "list", "--project", &sandbox.project_str()]);
    assert!(
        stdout(&out).contains(
            &sandbox
                .project()
                .join(".claude/skills")
                .to_string_lossy()
                .into_owned()
        )
    );
}

// ---------------------------------------------------------------- usage

#[test]
fn unknown_target_is_a_usage_error() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&[
        "skills",
        "install",
        "--target",
        "frobnicate",
        "--project",
        &sandbox.project_str(),
    ]);
    assert_eq!(out.status.code(), Some(2));
}

// ---------------------------------------------------------------- user scope

#[test]
fn user_install_writes_under_the_home_claude_skills() {
    let sandbox = Sandbox::new();
    sandbox.run(&["skills", "install", "--target", "claude", "--user"]);
    assert!(
        sandbox
            .home()
            .join(".claude/skills/crossmatrix/SKILL.md")
            .is_file()
    );
}
