//! Committed, SSH-signed approvals checked in a fresh CI clone.

use std::path::{Path, PathBuf};

use super::support::repo::{TestRepo, git_cmd};
use super::{func, git, onboard};

/// A key pair in its own directory; returns the private key path and its public key line.
fn keypair(dir: &Path, name: &str) -> (PathBuf, String) {
    let key = dir.join(name);
    let out = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"])
        .arg(&key)
        .output()
        .expect("ssh-keygen is installed");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    (key, public.trim().to_string())
}

struct Setup {
    dev: TestRepo,
    remote: tempfile::TempDir,
    keys: tempfile::TempDir,
    key: PathBuf,
}

/// main trusts `robert`'s key; `feature` adds an unapproved inline ignore.
fn setup() -> Setup {
    let keys = tempfile::tempdir().unwrap();
    let (key, public) = keypair(keys.path(), "robert");
    let dev = TestRepo::new();
    let remote = dev.add_remote();
    dev.write("a.rs", &func("small", 5));
    onboard(&dev);
    let cfg =
        dev.read("qgate.toml") + "\n[approvals]\nallowed_signers = \".qgate/allowed_signers\"\n";
    dev.write("qgate.toml", &cfg).write(
        ".qgate/allowed_signers",
        &format!("robert@example.com namespaces=\"qgate\" {public}\n"),
    );
    dev.add(&["qgate.toml", ".qgate/allowed_signers"]);
    dev.qgate().arg("approve").assert().success();
    let (ok, out) = git(&dev, &["commit", "-q", "-m", "trust robert"]);
    assert!(ok, "{out}");
    let (ok, out) = git(&dev, &["push", "-q", "--no-verify", "origin", "main"]);
    assert!(ok, "{out}");
    dev.git(&["checkout", "-q", "-b", "feature"]);
    dev.write(
        "a.rs",
        &("// eslint-disable\n".to_string() + &func("small", 5)),
    );
    dev.git(&["add", "a.rs"]);
    dev.git(&["commit", "-q", "--no-verify", "-m", "agent change"]);
    push_feature(&dev);
    Setup {
        dev,
        remote,
        keys,
        key,
    }
}

fn push_feature(dev: &TestRepo) {
    let (ok, out) = git(
        dev,
        &["push", "-q", "--no-verify", "-f", "origin", "feature"],
    );
    assert!(ok, "{out}");
    dev.git(&["fetch", "-q", "origin"]);
}

/// `qgate run --stage push --base origin/main` in a fresh full clone of `feature`.
fn ci(s: &Setup) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("file://{}", s.remote.path().display());
    let out = git_cmd(dir.path())
        .args(["clone", "-q", "--branch", "feature", &url, "."])
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = assert_cmd::Command::cargo_bin("qgate")
        .unwrap()
        .current_dir(dir.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["run", "--stage", "push", "--base", "origin/main"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    (out.status.success(), text)
}

fn sign(s: &Setup, key: &Path) -> serde_json::Value {
    let out = s
        .dev
        .qgate()
        .args([
            "--format",
            "json",
            "approve",
            "--base",
            "origin/main",
            "--sign",
            "--key",
        ])
        .arg(key)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn commit_record(s: &Setup) {
    s.dev.git(&["add", ".qgate/approvals"]);
    let (ok, out) = git(&s.dev, &["commit", "-q", "-m", "approve"]);
    assert!(ok, "{out}");
    push_feature(&s.dev);
}

#[test]
fn a_signed_record_approves_the_pr_in_a_fresh_clone() {
    let s = setup();
    let (ok, out) = ci(&s);
    assert!(!ok, "{out}");
    assert!(out.contains("unapproved-change"), "{out}");
    assert!(out.contains("fix: qgate approve --base "), "{out}");
    assert!(out.contains(" --sign"), "{out}");
    let r = sign(&s, &s.key);
    assert_eq!(r["data"]["status"], "signed", "{r:#}");
    assert_eq!(r["data"]["verified"], true, "{r:#}");
    commit_record(&s);
    let (ok, out) = ci(&s);
    assert!(ok, "{out}");
}

#[test]
fn a_forged_unsigned_record_fails() {
    let s = setup();
    let out = s
        .dev
        .qgate()
        .args([
            "--format",
            "json",
            "approve",
            "--base",
            "origin/main",
            "--dry-run",
        ])
        .output()
        .unwrap();
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let hash = r["data"]["approval"].as_str().unwrap().to_string();
    s.dev.write(
        &format!(".qgate/approvals/{hash}.sig"),
        "-----BEGIN SSH SIGNATURE-----\nforged\n-----END SSH SIGNATURE-----\n",
    );
    commit_record(&s);
    let (ok, out) = ci(&s);
    assert!(!ok, "{out}");
    assert!(out.contains("unapproved-change"), "{out}");
}

#[test]
fn a_signature_from_an_untrusted_key_fails() {
    let s = setup();
    let (other, _) = keypair(s.keys.path(), "agent");
    let r = sign(&s, &other);
    assert_eq!(r["data"]["verified"], false, "{r:#}");
    commit_record(&s);
    let (ok, out) = ci(&s);
    assert!(!ok, "{out}");
}

#[test]
fn a_pr_cannot_trust_its_own_new_key() {
    let s = setup();
    let (agent_key, agent_pub) = keypair(s.keys.path(), "agent");
    let signers = s.dev.read(".qgate/allowed_signers")
        + &format!("agent@example.com namespaces=\"qgate\" {agent_pub}\n");
    s.dev.write(".qgate/allowed_signers", &signers);
    s.dev.git(&["add", ".qgate/allowed_signers"]);
    s.dev
        .git(&["commit", "-q", "--no-verify", "-m", "trust me"]);
    push_feature(&s.dev);
    sign(&s, &agent_key);
    commit_record(&s);
    let (ok, out) = ci(&s);
    assert!(!ok, "{out}");
    assert!(
        out.contains("approvers changed: .qgate/allowed_signers"),
        "{out}"
    );
}
