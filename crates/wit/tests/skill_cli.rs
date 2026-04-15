use std::{fs, process::Command};

fn run_wit(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_wit"))
        .args(args)
        .output()
        .expect("failed to run wit")
}

fn expected_skill_markdown() -> String {
    let mut output = include_str!("../src/skill/SKILL.md").to_string();
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

#[test]
fn skill_load_prints_embedded_skill_markdown() {
    let output = run_wit(&["skill", "load"]);

    assert!(
        output.status.success(),
        "skill load failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        expected_skill_markdown()
    );
    assert!(output.stderr.is_empty(), "expected no stderr output");
}

#[test]
fn skill_install_creates_missing_parent_directories_and_writes_skill() {
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let install_root = temp.path().join("nested").join("skills");
    let install_root_arg = install_root.to_str().expect("path should be utf-8");

    let output = run_wit(&["skill", "install", "--path", install_root_arg]);

    assert!(
        output.status.success(),
        "skill install failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let skill_path = install_root.join("wit-skill").join("SKILL.md");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", skill_path.display())
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        format!("Installed wit skill to {}", skill_path.display())
    );
    assert_eq!(
        fs::read_to_string(&skill_path).expect("expected installed skill"),
        include_str!("../src/skill/SKILL.md")
    );
}

#[test]
fn skill_install_overwrites_existing_skill_file() {
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let install_root = temp.path().join("skills");
    let skill_dir = install_root.join("wit-skill");
    fs::create_dir_all(&skill_dir).expect("failed to create skill dir");
    let skill_path = skill_dir.join("SKILL.md");
    fs::write(&skill_path, "stale skill").expect("failed to seed stale skill");

    let output = run_wit(&[
        "skill",
        "install",
        "--path",
        install_root.to_str().expect("path should be utf-8"),
    ]);

    assert!(
        output.status.success(),
        "skill install failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&skill_path).expect("expected installed skill"),
        include_str!("../src/skill/SKILL.md")
    );
}

#[test]
fn skill_install_requires_path_flag_at_runtime() {
    let output = run_wit(&["skill", "install"]);

    assert!(
        !output.status.success(),
        "skill install should fail without --path"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("the following required arguments were not provided"));
    assert!(stderr.contains("--path <DIR>"));
}
