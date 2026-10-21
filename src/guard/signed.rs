//! Committed approvals (D16): `.qgate/approvals/<hash>.sig`, an SSH signature
//! (`ssh-keygen -Y sign`, namespace `qgate`) over the loosening text. CI can
//! check them offline. Only signers listed in `[approvals] allowed_signers`
//! of the trusted tree count, which is the tree before the change, so a change
//! can never approve itself by adding its own key.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::Loosening;
use crate::config::{CONFIG_FILE, Config};
use crate::git::{BlobReader, Repo, Snapshot};

pub const DIR: &str = ".qgate/approvals";
pub const NAMESPACE: &str = "qgate";

/// Whether `path` is a committed approval record, which never counts as a
/// change to check or guard.
pub fn is_record(path: &str) -> bool {
    path.strip_prefix(DIR)
        .is_some_and(|rest| rest.starts_with('/'))
}

pub fn record_path(hash: &str) -> String {
    format!("{DIR}/{hash}.sig")
}

/// The signers trusted for a change, and the signatures in the checked tree.
#[derive(Debug, Default)]
pub struct Committed {
    signers: Option<Vec<u8>>,
    sigs: HashMap<String, Vec<u8>>,
}

impl Committed {
    /// Signers from `trusted` (the tree before the change), signatures from `checked`.
    pub fn load(repo: &Repo, trusted: &Snapshot, checked: &Snapshot) -> Result<Committed> {
        let signers = Committed::signers_of(repo, trusted)?;
        let mut sigs = HashMap::new();
        if signers.is_some() {
            let mut blobs = BlobReader::new(repo)?;
            for (path, entry) in checked.files() {
                let hash = path
                    .strip_prefix(&format!("{DIR}/"))
                    .and_then(|n| n.strip_suffix(".sig"));
                if let Some(hash) = hash {
                    sigs.insert(hash.to_string(), blobs.read(&entry.oid)?);
                }
            }
        }
        Ok(Committed { signers, sigs })
    }

    /// The allowed_signers content configured in `snapshot`, if any.
    pub fn signers_of(repo: &Repo, snapshot: &Snapshot) -> Result<Option<Vec<u8>>> {
        let mut blobs = BlobReader::new(repo)?;
        let Some(cfg) = snapshot.get(CONFIG_FILE) else {
            return Ok(None);
        };
        let text = String::from_utf8_lossy(&blobs.read(&cfg.oid)?).into_owned();
        let cfg = Config::parse_unchecked(&text, CONFIG_FILE)?;
        match cfg
            .approvals
            .allowed_signers
            .and_then(|p| snapshot.get(&p).cloned())
        {
            Some(entry) => Ok(Some(blobs.read(&entry.oid)?)),
            None => Ok(None),
        }
    }

    pub fn insert(&mut self, hash: &str, sig: Vec<u8>) {
        self.sigs.insert(hash.to_string(), sig);
    }

    pub fn with_signers(mut self, signers: Vec<u8>) -> Self {
        self.signers = Some(signers);
        self
    }

    pub fn has_signers(&self) -> bool {
        self.signers.is_some()
    }

    /// Whether a committed record approves `l`, signed by a trusted signer.
    pub fn approves(&self, l: &Loosening) -> bool {
        let (Some(signers), Some(sig)) = (&self.signers, self.sigs.get(&l.hash())) else {
            return false;
        };
        verify(signers, sig, l.text().as_bytes()).unwrap_or(false)
    }
}

/// Whether `sig` is a valid `qgate` signature over `data` by a principal in `signers`.
pub fn verify(signers: &[u8], sig: &[u8], data: &[u8]) -> Result<bool> {
    let dir = tempfile::tempdir()?;
    let signers_path = dir.path().join("allowed_signers");
    let sig_path = dir.path().join("approval.sig");
    std::fs::write(&signers_path, signers)?;
    std::fs::write(&sig_path, sig)?;
    let out = ssh_keygen()
        .args(["-Y", "find-principals", "-s"])
        .arg(&sig_path)
        .arg("-f")
        .arg(&signers_path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("running ssh-keygen -Y find-principals")?;
    if !out.status.success() {
        return Ok(false);
    }
    for principal in String::from_utf8_lossy(&out.stdout).lines() {
        let principal = principal.trim();
        if principal.is_empty() {
            continue;
        }
        let ok = run_with_stdin(
            ssh_keygen()
                .args(["-Y", "verify", "-n", NAMESPACE, "-I", principal, "-f"])
                .arg(&signers_path)
                .arg("-s")
                .arg(&sig_path),
            data,
        )?
        .status
        .success();
        if ok {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Sign `data` with `key` (a private key, or a public key held by ssh-agent).
pub fn sign(key: &Path, data: &[u8]) -> Result<Vec<u8>> {
    let out = run_with_stdin(
        ssh_keygen()
            .args(["-Y", "sign", "-n", NAMESPACE, "-f"])
            .arg(key),
        data,
    )?;
    if !out.status.success() {
        bail!(
            "ssh-keygen -Y sign failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

fn ssh_keygen() -> Command {
    let mut cmd = Command::new("ssh-keygen");
    cmd.stdout(Stdio::piped());
    cmd
}

fn run_with_stdin(cmd: &mut Command, data: &[u8]) -> Result<std::process::Output> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running ssh-keygen (is OpenSSH installed?)")?;
    let mut stdin = child.stdin.take().context("ssh-keygen stdin")?;
    let data = data.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&data));
    let out = child.wait_with_output()?;
    writer
        .join()
        .expect("writer thread")
        .context("writing to ssh-keygen")?;
    Ok(out)
}
