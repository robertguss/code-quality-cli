//! `qgate update`: replace this binary with a GitHub release, using the same
//! archives and checksums as install.sh. It shells out to curl and tar, which
//! install.sh needs too, so the binary carries no HTTP or TLS stack.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::cli::{Finding, Report, UsageError};

const RELEASES: &str = "https://github.com/robertguss/qgate/releases";
/// Overrides `RELEASES`, for mirrors and tests. `file://` URLs work with `--tag`.
const RELEASES_ENV: &str = "QGATE_RELEASES_URL";
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const AVAILABLE: &str = "update-available";
pub const REFRESH_SKILL: &str = "refresh-skill";

pub struct Options<'a> {
    /// Report whether an update exists without installing it.
    pub check: bool,
    /// Install this release instead of the latest, newer or older.
    pub tag: Option<&'a str>,
}

pub fn update(opts: &Options) -> Result<Report> {
    let started = Instant::now();
    let base = std::env::var(RELEASES_ENV).unwrap_or_else(|_| RELEASES.to_string());
    let base = base.trim_end_matches('/');
    let current = format!("v{VERSION}");
    let target = match opts.tag {
        Some(tag) => normalize_tag(tag)?,
        None => latest_tag(base)?,
    };
    let newer = is_newer(&target, &current)?;

    let mut report = Report::new("update");
    report.set("current", current.as_str());
    report.set(
        if opts.tag.is_some() {
            "target"
        } else {
            "latest"
        },
        target.as_str(),
    );
    report.set("update_available", newer);

    if opts.check {
        if newer {
            report.extend([Finding::info(
                AVAILABLE,
                format!("qgate {target} is available; this is {current}"),
            )
            .with_fix(fix_command(opts.tag, &target))]);
        }
        report.timing("total", started.elapsed());
        return Ok(report.finish());
    }

    // Without --tag, only move forward: a local build ahead of the latest
    // release stays put.
    if target == current || (opts.tag.is_none() && !newer) {
        report.set("installed", false);
        report.timing("total", started.elapsed());
        return Ok(report.finish());
    }

    let exe = current_exe()?;
    install(base, &target, &exe)?;
    report.set("installed", true);
    report.set("path", exe.display().to_string());
    report.extend([Finding::info(
        REFRESH_SKILL,
        format!(
            "qgate is now {target}; run `qgate install` in each repo to refresh its agent skill"
        ),
    )
    .with_fix("qgate install")]);
    report.timing("total", started.elapsed());
    Ok(report.finish())
}

/// The command that installs `target`: plain `qgate update` for the latest.
fn fix_command(tag: Option<&str>, target: &str) -> String {
    match tag {
        Some(_) => format!("qgate update --tag {target}"),
        None => "qgate update".to_string(),
    }
}

/// `0.3.0` and `v0.3.0` both name release `v0.3.0`.
fn normalize_tag(tag: &str) -> Result<String> {
    let tag = tag.trim();
    let tag = if tag.starts_with('v') {
        tag.to_string()
    } else {
        format!("v{tag}")
    };
    if parse_version(&tag).is_none() {
        return Err(UsageError::new(format!("--tag {tag} is not a version"))
            .with_hint("qgate update --tag v0.2.0")
            .into());
    }
    Ok(tag)
}

/// Ask GitHub where `releases/latest` redirects: `.../releases/tag/v0.3.0`.
/// The redirect needs no API token and has no rate limit.
fn latest_tag(base: &str) -> Result<String> {
    let url = format!("{base}/latest");
    let out = Command::new("curl")
        .args(["-fsSIL", "-o", "/dev/null", "-w", "%{url_effective}", &url])
        .output()
        .context("could not run curl; qgate update needs curl and tar on PATH")?;
    if !out.status.success() {
        bail!(
            "could not reach {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let effective = String::from_utf8_lossy(&out.stdout);
    tag_from_url(&effective)
        .with_context(|| format!("{url} did not redirect to a release tag (got {effective})"))
}

fn tag_from_url(url: &str) -> Option<String> {
    let tag = url.trim().rsplit_once("/tag/")?.1.trim_end_matches('/');
    parse_version(tag)?;
    Some(tag.to_string())
}

/// `v1.2.3` as numbers. A pre-release suffix (`-rc.1`) is ignored.
fn parse_version(tag: &str) -> Option<Vec<u64>> {
    let core = tag.strip_prefix('v')?.split(['-', '+']).next()?;
    core.split('.').map(|n| n.parse().ok()).collect()
}

fn is_newer(target: &str, current: &str) -> Result<bool> {
    let parse = |t: &str| parse_version(t).with_context(|| format!("{t} is not a version"));
    Ok(parse(target)? > parse(current)?)
}

/// The release archive for the platform this binary was built for.
fn asset_name() -> Result<String> {
    let os = match std::env::consts::OS {
        "linux" => "unknown-linux-musl",
        "macos" => "apple-darwin",
        other => bail!("no qgate release for {other}; build from source with cargo install"),
    };
    let arch = match std::env::consts::ARCH {
        a @ ("x86_64" | "aarch64") => a,
        other => bail!("no qgate release for {other}; build from source with cargo install"),
    };
    Ok(format!("qgate-{arch}-{os}.tar.gz"))
}

/// The real file behind this process, through any symlinks.
fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("could not find the running qgate binary")?;
    std::fs::canonicalize(&exe).with_context(|| format!("could not resolve {}", exe.display()))
}

/// Download, verify and unpack `tag`, then swap it in for `exe` atomically.
/// Git hooks call qgate by absolute path, so they keep working.
fn install(base: &str, tag: &str, exe: &Path) -> Result<()> {
    let asset = asset_name()?;
    let tmp = tempfile::tempdir()?;
    let archive = tmp.path().join(&asset);
    let sums = tmp.path().join(format!("{asset}.sha256"));
    let url = format!("{base}/download/{tag}/{asset}");
    download(&url, &archive)?;
    download(&format!("{url}.sha256"), &sums)?;

    let expected = std::fs::read_to_string(&sums)?;
    let expected = expected.split_whitespace().next().unwrap_or_default();
    let actual = crate::skill::digest(&std::fs::read(&archive)?);
    if expected != actual {
        bail!("checksum mismatch for {asset} {tag}: expected {expected}, got {actual}");
    }

    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(tmp.path())
        .arg("qgate")
        .status()
        .context("could not run tar")?;
    if !status.success() {
        bail!("could not unpack {asset}");
    }
    let unpacked = tmp.path().join("qgate");
    check_runs(&unpacked, tag)?;

    let dir = exe
        .parent()
        .context("the qgate binary has no parent directory")?;
    let staged = tempfile::Builder::new()
        .prefix(".qgate-update-")
        .tempfile_in(dir)
        .with_context(|| format!("cannot write to {}", dir.display()))?;
    std::fs::copy(&unpacked, staged.path())?;
    set_executable(staged.path())?;
    staged
        .persist(exe)
        .with_context(|| format!("could not replace {}", exe.display()))?;
    Ok(())
}

fn download(url: &str, to: &Path) -> Result<()> {
    let out = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(to)
        .arg(url)
        .output()
        .context("could not run curl; qgate update needs curl and tar on PATH")?;
    if !out.status.success() {
        bail!(
            "could not download {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Run the new binary once, so a broken or wrong-platform download never
/// replaces a working one.
fn check_runs(binary: &Path, tag: &str) -> Result<()> {
    set_executable(binary)?;
    let out = Command::new(binary)
        .arg("--version")
        .output()
        .with_context(|| format!("the downloaded qgate {tag} does not run here"))?;
    let reported = String::from_utf8_lossy(&out.stdout);
    let want = format!("qgate {}", tag.trim_start_matches('v'));
    if !out.status.success() || reported.trim() != want {
        bail!(
            "the downloaded qgate {tag} reported {:?}, expected {want:?}",
            reported.trim()
        );
    }
    Ok(())
}

fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_comes_from_the_latest_redirect() {
        assert_eq!(
            tag_from_url("https://github.com/robertguss/qgate/releases/tag/v0.3.0\n"),
            Some("v0.3.0".to_string())
        );
        assert_eq!(
            tag_from_url("https://github.com/robertguss/qgate/releases"),
            None
        );
        assert_eq!(tag_from_url("https://x/releases/tag/nightly"), None);
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("v0.10.0", "v0.9.1").unwrap());
        assert!(is_newer("v1.0.0", "v0.2.0").unwrap());
        assert!(!is_newer("v0.2.0", "v0.2.0").unwrap());
        assert!(!is_newer("v0.1.9", "v0.2.0").unwrap());
        assert!(is_newer("v0.3.0-rc.1", "v0.2.0").unwrap());
        assert!(is_newer("nope", "v0.2.0").is_err());
    }

    #[test]
    fn tags_get_a_v() {
        assert_eq!(normalize_tag("0.2.0").unwrap(), "v0.2.0");
        assert_eq!(normalize_tag("v0.2.0").unwrap(), "v0.2.0");
        assert!(normalize_tag("latest").is_err());
    }
}
