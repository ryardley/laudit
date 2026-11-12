use std::process::{Command, Output};

fn laudit(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_laudit"))
        .args(args)
        .output()
        .expect("run laudit")
}

#[test]
fn help_lists_every_subcommand() {
    let output = laudit(&["--help"]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in [
        "build", "serve", "tour", "explain", "export", "ask", "mcp", "outline", "status",
        "prompts", "clear",
    ] {
        assert!(stdout.contains(command), "missing {command} in:\n{stdout}");
    }
}

#[test]
fn version_prints_the_crate_version() {
    let output = laudit(&["--version"]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "got: {stdout}");
}

#[test]
fn unknown_flags_are_rejected() {
    let output = laudit(&["build", "--bogus"]);
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument"), "got: {stderr}");
}

#[test]
fn stubs_parse_documented_flags_and_exit_non_zero() {
    let cases: &[&[&str]] = &[
        &[
            "build",
            "--offline",
            "--check",
            "--quiet",
            "--repo",
            ".",
            "--json",
        ],
        &[
            "tour",
            "--print",
            "--uncommitted",
            "--range",
            "main..HEAD",
            "--per-commit",
            "--color",
            "never",
        ],
        &["explain", "some_symbol"],
        &["export", "--md", "--out", "tour.md"],
        &["export", "--html"],
        &["ask", "what does this do?"],
        &["mcp"],
        &["outline"],
        &["status"],
        &["clear"],
    ];

    for args in cases {
        let output = laudit(args);
        assert!(
            !output.status.success(),
            "expected non-zero exit for {args:?}"
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("not implemented yet"),
            "missing stub message for {args:?}: {stderr}"
        );
    }
}

#[test]
fn export_requires_a_format() {
    let output = laudit(&["export"]);
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--md"), "got: {stderr}");
    assert!(stderr.contains("--html"), "got: {stderr}");
}
