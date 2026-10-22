//! Exclusions that hide always-block occurrences (KTD15): a new `exclude`
//! glob, a `.gitattributes` line marking files generated or vendored, or a Go
//! generated-code header takes a file out of every count, so it blocks when
//! the file holds an always-block occurrence before or after the change.

use anyhow::Result;

use super::{Configs, Found};
use crate::checks::exclude::{self, Excludes};
use crate::config::CONFIG_FILE;
use crate::git::{BlobReader, Change, Repo, Snapshot};
use crate::grammars::{self, Lang};
use crate::rules::{self, engine::RuleSet};

/// Flags each file that the change newly excludes while it holds always-block
/// occurrences, on the parent or the current side.
pub(super) fn flag_hidden(
    repo: &Repo,
    parent: &Snapshot,
    current: &Snapshot,
    changes: &[Change],
    configs: &Configs,
    found: &mut Found,
) -> Result<()> {
    let mut paths = generated_now(repo, parent, current, changes)?;
    let rules_changed = changes
        .iter()
        .any(|c| c.path == CONFIG_FILE || c.path.ends_with(".gitattributes"));
    if rules_changed {
        let before = Excludes::new(repo, parent, &configs.parent)?;
        let after = Excludes::new(repo, current, &configs.new)?;
        paths.extend(
            current
                .files()
                .chain(parent.files())
                .map(|(p, _)| p)
                .filter(|p| after.is_excluded(p) && !before.is_excluded(p))
                .cloned(),
        );
    }
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return Ok(());
    }
    let ruleset = RuleSet::from_config(&configs.new);
    let mut reader = BlobReader::new(repo)?;
    for path in paths {
        let mut hidden = 0;
        for snapshot in [parent, current] {
            if let Some(e) = snapshot.get(&path).filter(|e| e.is_file()) {
                let text = String::from_utf8_lossy(&reader.read(&e.oid)?).into_owned();
                hidden += always_block_count(&path, &text, &ruleset, &configs.always_block);
            }
        }
        if hidden > 0 {
            found.flag(
                &path,
                format!("exclusion: newly excluded {path} holds always-block occurrences"),
                true,
            );
        }
    }
    Ok(())
}

/// Changed Go files that now carry a generated-code header.
fn generated_now(
    repo: &Repo,
    parent: &Snapshot,
    current: &Snapshot,
    changes: &[Change],
) -> Result<Vec<String>> {
    let mut reader = BlobReader::new(repo)?;
    let mut text = |s: &Snapshot, p: &str| -> Result<Option<String>> {
        match s.get(p).filter(|e| e.is_file()) {
            Some(e) => Ok(Some(
                String::from_utf8_lossy(&reader.read(&e.oid)?).into_owned(),
            )),
            None => Ok(None),
        }
    };
    let mut out = Vec::new();
    for c in changes
        .iter()
        .filter(|c| !c.is_deleted() && c.path.ends_with(".go"))
    {
        let now = text(current, &c.path)?.is_some_and(|t| exclude::is_generated(&c.path, &t));
        let before = text(parent, &c.path)?.is_some_and(|t| exclude::is_generated(&c.path, &t));
        if now && !before {
            out.push(c.path.clone());
        }
    }
    Ok(out)
}

/// Always-block occurrences in one file's text, ignoring every exclusion.
fn always_block_count(path: &str, text: &str, ruleset: &RuleSet, always_block: &[String]) -> usize {
    let Some(lang) = Lang::detect(path) else {
        return 0;
    };
    let Some(parsed) = grammars::parse(lang, text) else {
        return 0;
    };
    rules::engine::scan(path, lang, &parsed.tree, text, ruleset)
        .iter()
        .filter_map(|o| rules::find(o.rule))
        .filter(|r| always_block.iter().any(|n| rules::always_blocked(n, r)))
        .count()
}
