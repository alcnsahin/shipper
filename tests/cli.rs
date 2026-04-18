//! Integration smoke tests for the `shipper` binary.
//!
//! These exercise only the CLI surface that does not touch the network,
//! external tools, or project-specific config. Deeper pipeline tests land
//! in Faz 3 (resume/dry-run) and Faz 6 (subprocess helper mocks).

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn prints_version() {
    Command::cargo_bin("shipper")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_lists_deploy_init_validate() {
    Command::cargo_bin("shipper")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("deploy"))
        .stdout(contains("init"))
        .stdout(contains("validate"));
}

#[test]
fn deploy_help_lists_targets_and_dry_run() {
    Command::cargo_bin("shipper")
        .unwrap()
        .args(["deploy", "--help"])
        .assert()
        .success()
        .stdout(contains("ios"))
        .stdout(contains("android"))
        .stdout(contains("all"))
        .stdout(contains("--dry-run"));
}
