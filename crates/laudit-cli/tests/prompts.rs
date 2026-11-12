use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "laudit-cli-prompts-test-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn laudit(home: &TempDir, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_laudit"))
        .args(args)
        .env("HOME", &home.path)
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("run laudit")
}

#[test]
fn prompts_installs_on_first_run_and_lists_them() {
    let home = TempDir::new();
    let output = laudit(&home, &["prompts"]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in ["comprehension", "narration", "repair", "ask"] {
        assert!(stdout.contains(name), "missing {name} in:\n{stdout}");
    }

    let prompts = home.path.join(".config/laudit/prompts");
    for name in ["comprehension", "narration", "repair", "ask"] {
        assert!(
            prompts.join(format!("{name}.md")).is_file(),
            "missing {name}.md"
        );
    }
    assert!(prompts.join("manifest.json").is_file());
}

#[test]
fn prompts_prints_a_named_prompt() {
    let home = TempDir::new();
    let output = laudit(&home, &["prompts", "narration"]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("TITLE:"));
    assert!(stdout.contains("WHY:"));
}

#[test]
fn prompts_all_prints_every_prompt() {
    let home = TempDir::new();
    let output = laudit(&home, &["prompts", "--all"]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in ["comprehension", "narration", "repair", "ask"] {
        assert!(stdout.contains(&format!("===== {name} =====")));
    }
}

#[test]
fn prompts_uses_edited_installed_prompt() {
    let home = TempDir::new();
    let first = laudit(&home, &["prompts"]);
    assert!(first.status.success());

    let path = home.path.join(".config/laudit/prompts/comprehension.md");
    fs::write(&path, "edited prompt body\n").expect("edit prompt");

    let output = laudit(&home, &["prompts", "comprehension"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "edited prompt body\n"
    );
}

#[test]
fn prompts_rejects_unknown_names() {
    let home = TempDir::new();
    let output = laudit(&home, &["prompts", "nope"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown prompt"));
}

#[test]
fn prompts_warns_when_a_newer_shipped_version_exists() {
    let home = TempDir::new();
    let first = laudit(&home, &["prompts", "--list"]);
    assert!(first.status.success());

    let manifest_path = home.path.join(".config/laudit/prompts/manifest.json");
    let contents = fs::read_to_string(&manifest_path).expect("read manifest");
    let mut manifest: serde_json::Value = serde_json::from_str(&contents).expect("parse manifest");
    manifest["prompts"]["comprehension"] = serde_json::json!(0);
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write manifest");

    let output = laudit(&home, &["prompts", "--list"]);
    assert!(output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("newer shipped version"),
        "missing warning in stderr: {stderr}"
    );
    assert!(home
        .path
        .join(".config/laudit/prompts/comprehension.md.new")
        .is_file());
}
