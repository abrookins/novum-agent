use std::fs;
use std::process::Command;

use anyhow::Context;
use anyhow::Result;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[test]
fn packaged_version_reports_release_and_compiled_commit() -> Result<()> {
    let package = TempDir::new()?;
    let bin_dir = package.path().join("bin");
    fs::create_dir(&bin_dir)?;
    let binary = bin_dir.join(format!("codex{}", std::env::consts::EXE_SUFFIX));
    let source = codex_utils_cargo_bin::cargo_bin("codex")?;
    let source_version = Command::new(&source).arg("--version").output()?;
    assert!(source_version.status.success());
    let source_version = String::from_utf8(source_version.stdout)?;
    let (_, provenance) = source_version
        .split_once(" (Novum; commit ")
        .context("source binary reports compiled commit")?;
    fs::copy(source, &binary)?;
    fs::write(
        package.path().join("codex-package.json"),
        r#"{"version":"0.1.0"}"#,
    )?;

    let short = Command::new(&binary).arg("-V").output()?;
    assert!(short.status.success());
    assert_eq!(String::from_utf8(short.stdout)?, "codex-cli 0.1.0\n");

    let long = Command::new(&binary).arg("--version").output()?;
    assert!(long.status.success());
    let long = String::from_utf8(long.stdout)?;
    assert_eq!(long, format!("codex-cli 0.1.0 (Novum; commit {provenance}"));
    insta::assert_snapshot!(
        "packaged_release_version",
        long.replace(provenance.trim_end(), "[COMMIT])")
    );
    Ok(())
}
