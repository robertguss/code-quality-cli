//! `qgate update` against a fake release served from `file://`, run from a
//! copy of the binary so the copy is what gets replaced.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;
use predicates::prelude::*;

struct Fixture {
    _dir: tempfile::TempDir,
    bin: PathBuf,
    releases: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin/qgate");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::copy(cargo_bin("qgate"), &bin).unwrap();
        wait_until_runnable(&bin);
        let releases = dir.path().join("releases");
        Fixture {
            _dir: dir,
            bin,
            releases,
        }
    }

    /// Publish release `tag` whose binary is a script printing `prints`.
    fn publish(&self, tag: &str, prints: &str) -> PathBuf {
        let out = self.releases.join("download").join(tag);
        let stage = out.join("stage");
        std::fs::create_dir_all(&stage).unwrap();
        let script = stage.join("qgate");
        std::fs::write(&script, format!("#!/bin/sh\necho '{prints}'\n")).unwrap();
        let asset = asset();
        let archive = out.join(&asset);
        let ok = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&stage)
            .arg("qgate")
            .status()
            .unwrap()
            .success();
        assert!(ok, "tar failed");
        let sum = qgate::skill::digest(&std::fs::read(&archive).unwrap());
        std::fs::write(
            out.join(format!("{asset}.sha256")),
            format!("{sum}  {asset}\n"),
        )
        .unwrap();
        archive
    }

    fn update(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        assert_cmd::Command::new(&self.bin)
            .arg("update")
            .args(args)
            .env(
                "QGATE_RELEASES_URL",
                format!("file://{}", self.releases.display()),
            )
            .assert()
    }

    fn version(&self) -> String {
        let out = Command::new(&self.bin).arg("--version").output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }
}

/// Another test thread can fork while this one still has the copy open for
/// writing, and its child holds that descriptor until it execs. Running the
/// copy fails with "text file busy" until then.
fn wait_until_runnable(bin: &Path) {
    for _ in 0..200 {
        match Command::new(bin).arg("--version").output() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => {
                other.unwrap();
                return;
            }
        }
    }
    panic!("{} stayed busy", bin.display());
}

fn asset() -> String {
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        _ => "unknown-linux-musl",
    };
    format!("qgate-{}-{os}.tar.gz", std::env::consts::ARCH)
}

fn current() -> String {
    format!("qgate {}", env!("CARGO_PKG_VERSION"))
}

#[test]
fn installs_the_tagged_release_over_itself() {
    let f = Fixture::new();
    f.publish("v99.0.0", "qgate 99.0.0");
    f.update(&["--tag", "99.0.0", "--format", "json"])
        .success()
        .stdout(predicate::str::contains("\"installed\": true"))
        .stdout(predicate::str::contains("\"fix\": \"qgate install\""));
    assert_eq!(f.version(), "qgate 99.0.0");
}

#[test]
fn check_reports_without_installing() {
    let f = Fixture::new();
    f.update(&["--check", "--tag", "v99.0.0"])
        .success()
        .stdout(predicate::str::contains("update_available: true"))
        .stdout(predicate::str::contains("fix: qgate update --tag v99.0.0"));
    assert_eq!(f.version(), current());
}

#[test]
fn checksum_mismatch_keeps_the_old_binary() {
    let f = Fixture::new();
    let archive = f.publish("v99.0.0", "qgate 99.0.0");
    std::fs::write(&archive, b"tampered").unwrap();
    f.update(&["--tag", "v99.0.0"])
        .code(2)
        .stderr(predicate::str::contains("checksum mismatch"));
    assert_eq!(f.version(), current());
}

#[test]
fn a_binary_reporting_the_wrong_version_is_refused() {
    let f = Fixture::new();
    f.publish("v99.0.0", "something else");
    f.update(&["--tag", "v99.0.0"])
        .code(2)
        .stderr(predicate::str::contains("expected \"qgate 99.0.0\""));
    assert_eq!(f.version(), current());
    assert_no_leftovers(f.bin.parent().unwrap());
}

#[test]
fn a_missing_release_fails_with_its_url() {
    let f = Fixture::new();
    f.update(&["--tag", "v99.0.0"])
        .code(2)
        .stderr(predicate::str::contains("could not download"));
    assert_eq!(f.version(), current());
}

#[test]
fn a_bad_tag_is_a_usage_error() {
    let f = Fixture::new();
    f.update(&["--tag", "latest"])
        .code(2)
        .stderr(predicate::str::contains("qgate update --tag v0.2.0"));
}

fn assert_no_leftovers(dir: &Path) {
    let names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["qgate"]);
}
