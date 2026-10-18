//! Three-way merge of `qgate.baseline`, run by git as a merge driver:
//! the lower value wins, keys removed on either side stay removed, and keys
//! added on one side are kept.

use std::path::Path;

use anyhow::{Context, Result};

use super::Baseline;

pub fn merge(base: &Baseline, ours: &Baseline, theirs: &Baseline) -> Baseline {
    let mut out = Baseline::default();
    let keys = ours.entries.keys().chain(theirs.entries.keys());
    for key in keys {
        if out.entries.contains_key(key) {
            continue;
        }
        let (o, t) = (ours.get(key), theirs.get(key));
        let value = match (base.get(key), o, t) {
            (_, Some(o), Some(t)) => o.min(t),
            // Present before, removed (pruned) on one side.
            (Some(_), _, _) => continue,
            (None, Some(v), None) | (None, None, Some(v)) => v,
            (None, None, None) => continue,
        };
        out.entries.insert(key.clone(), value);
    }
    out
}

/// git's `%O %A %B`: read all three, write the result to `ours`. Returns the
/// driver's exit code: 0 merged, 1 conflict (a file did not parse).
pub fn run(base: &Path, ours: &Path, theirs: &Path) -> Result<i32> {
    let read = |p: &Path| -> Result<Option<Baseline>> {
        let text =
            std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
        Ok(Baseline::parse(&text).ok())
    };
    let (Some(b), Some(o), Some(t)) = (read(base)?, read(ours)?, read(theirs)?) else {
        eprintln!("qgate: a side of qgate.baseline does not parse; resolve the conflict by hand");
        return Ok(1);
    };
    std::fs::write(ours, merge(&b, &o, &t).render())?;
    Ok(0)
}
