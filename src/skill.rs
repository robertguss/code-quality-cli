//! The agent skill bundled in the binary. `qgate --skill` prints it,
//! `qgate install` writes it into the repo, and `qgate run` checks the
//! committed copies: an older release's copy warns, anything else fails, so an
//! agent can't rewrite the instructions later agents read.

use anyhow::Result;

use crate::cli::Finding;
use crate::git::{BlobReader, Repo, Snapshot};

/// Where agents look for skills: Claude Code, then Codex and other agents
/// that read `.agents/skills`.
pub const PATHS: [&str; 2] = [
    ".claude/skills/qgate/SKILL.md",
    ".agents/skills/qgate/SKILL.md",
];

const TEMPLATE: &str = include_str!("../skill/SKILL.md");
/// `<version> <sha256 of that version's rendered skill>`, one per line.
const HISTORY: &str = include_str!("../skill/history.txt");
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const EDITED: &str = "skill-edited";
pub const OUTDATED: &str = "skill-outdated";
pub const MISSING: &str = "skill-missing";

/// The skill for this version of qgate.
pub fn render() -> String {
    TEMPLATE.replace("{{version}}", VERSION)
}

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The releases recorded in `history`, with the digests of their skills.
fn releases(history: &str) -> impl Iterator<Item = (&str, &str)> {
    history
        .lines()
        .filter_map(|l| l.trim().split_once(' '))
        .map(|(v, d)| (v.trim(), d.trim()))
}

/// How a committed copy compares with this binary's skill.
#[derive(Debug, PartialEq, Eq)]
pub enum CopyState<'a> {
    Current,
    /// Unedited, from the release named.
    Outdated(&'a str),
    /// Matches no release qgate knows.
    Edited,
}

pub fn classify<'a>(bytes: &[u8], current: &str, history: &'a str) -> CopyState<'a> {
    if bytes == current.as_bytes() {
        return CopyState::Current;
    }
    let sum = digest(bytes);
    match releases(history).find(|(_, d)| *d == sum) {
        Some((v, _)) => CopyState::Outdated(v),
        None => CopyState::Edited,
    }
}

/// The committed copies in `snapshot` that don't match this binary's skill.
pub fn check(repo: &Repo, snapshot: &Snapshot) -> Result<Vec<Finding>> {
    let current = render();
    let mut blobs = BlobReader::new(repo)?;
    let mut out = Vec::new();
    for path in PATHS {
        let refresh = format!("qgate install && git add {path}");
        let Some(entry) = snapshot.get(path) else {
            let fix = if repo.root.join(path).exists() {
                format!("git add {path}")
            } else {
                refresh
            };
            out.push(
                Finding::warning(MISSING, "the qgate agent skill is not committed")
                    .at(path, None)
                    .with_fix(fix),
            );
            continue;
        };
        let bytes = blobs.read(&entry.oid)?;
        let finding = match classify(&bytes, &current, HISTORY) {
            CopyState::Current => continue,
            CopyState::Outdated(v) => Finding::warning(
                OUTDATED,
                format!(
                    "the qgate skill here is from v{v} and this qgate is v{VERSION}; run `qgate install` to update it"
                ),
            ),
            CopyState::Edited => Finding::error(
                EDITED,
                format!(
                    "the qgate skill differs from every copy qgate v{VERSION} knows, so it was edited by hand \
                     (or written by a newer qgate; then update qgate instead)"
                ),
            ),
        };
        out.push(finding.at(path, None).with_fix(refresh));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_records_this_version() {
        let sum = digest(render().as_bytes());
        let recorded = releases(HISTORY)
            .find(|(v, _)| *v == VERSION)
            .map(|(_, d)| d);
        assert_eq!(
            recorded,
            Some(sum.as_str()),
            "skill/history.txt must end with `{VERSION} {sum}`. Replace the line for \
             {VERSION} if it is there; keep the lines of released versions."
        );
    }

    #[test]
    fn an_older_release_is_outdated_and_anything_else_is_edited() {
        let old = "old skill\n";
        let history = format!("0.1.0 {}\n", digest(old.as_bytes()));
        assert_eq!(classify(b"now", "now", &history), CopyState::Current);
        assert_eq!(
            classify(old.as_bytes(), "now", &history),
            CopyState::Outdated("0.1.0")
        );
        assert_eq!(
            classify(b"old skill, edited\n", "now", &history),
            CopyState::Edited
        );
    }

    #[test]
    fn rendered_skill_names_this_version() {
        let text = render();
        assert!(text.starts_with("---\nname: qgate\n"), "{text}");
        assert!(text.contains(&format!("<!-- qgate {VERSION}.")), "{text}");
        assert!(!text.contains("{{"), "unfilled placeholder");
    }
}
