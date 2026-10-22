//! `qgate run`: the whole pipeline for a commit or a push, ending in one result.

use std::io::Read;
use std::time::Instant;

use anyhow::Result;

use crate::baseline::{self, compare};
use crate::cli::{Report, Severity, UsageError, progress};
use crate::config::{self, Stage};
use crate::git::{Change, PushRef, Repo, Snapshot, Source, ZERO_OID, changes};
use crate::guard;
use crate::tools::{self, ToolContext};

pub fn run(
    repo: &Repo,
    stage: Stage,
    remote: &str,
    hook_input: bool,
    base: Option<&str>,
) -> Result<Report> {
    let start = Instant::now();
    if base.is_some() && stage != Stage::Push {
        return Err(UsageError::new("--base only applies to the push stage")
            .with_hint("qgate run --stage push --base origin/main")
            .into());
    }
    let mut report = Report::new(&format!("run --stage {}", stage.as_str()));
    match stage {
        Stage::Commit => commit(repo, &mut report)?,
        Stage::Push => {
            let refs = if hook_input {
                let mut input = String::new();
                std::io::stdin().read_to_string(&mut input)?;
                changes::parse_pre_push(&input)
            } else {
                head_ref(repo)
            };
            for r in refs.iter().filter(|r| !r.is_delete()) {
                let base = push_base(repo, remote, r, base)?;
                push(repo, base, r, refs.len() > 1, &mut report)?;
            }
        }
    }
    report.timing("total", start.elapsed());
    Ok(report.finish())
}

fn plural(n: usize, word: &str) -> String {
    match n {
        1 => word.to_string(),
        _ => format!("{word}s"),
    }
}

fn head_ref(repo: &Repo) -> Vec<PushRef> {
    repo.head()
        .map(|head| PushRef {
            local_ref: "HEAD".into(),
            local_oid: head,
            remote_ref: String::new(),
            remote_oid: ZERO_OID.into(),
        })
        .into_iter()
        .collect()
}

fn commit(repo: &Repo, report: &mut Report) -> Result<()> {
    let changes = changes::staged(repo)?;
    report.set("files", changes.len());
    if changes.is_empty() {
        return Ok(());
    }
    progress::line(format!(
        "checking {} staged {}",
        changes.len(),
        plural(changes.len(), "file")
    ));
    let current = Snapshot::index(repo)?;
    let parent = Snapshot::at(repo, repo.head().as_deref())?;
    let t = Instant::now();
    let l = guard::loosening(repo, &parent, &current, &changes)?;
    report.extend(guard::check(&l));
    report.timing("guard", t.elapsed());
    check_tree(
        repo,
        Stage::Commit,
        Source::Index,
        &current,
        &parent,
        &changes,
        report,
    )?;
    Ok(())
}

/// What a pushed ref is checked against. In a shallow clone a missing merge
/// base is an error, not the empty tree, which would make every file look new.
fn push_base(
    repo: &Repo,
    remote: &str,
    r: &PushRef,
    explicit: Option<&str>,
) -> Result<Option<String>> {
    const DEEPEN: &str =
        "fetch full history (actions/checkout: fetch-depth: 0), or git fetch --unshallow";
    let shallow = repo
        .try_output(&["rev-parse", "--is-shallow-repository"])
        .as_deref()
        == Some("true");
    let Some(rev) = explicit else {
        let base = changes::push_base(repo, remote, r);
        if base.is_none() && shallow {
            return Err(UsageError::new(
                "shallow clone: no merge base with the remote default branch, so there is nothing to compare against",
            )
            .with_hint(DEEPEN)
            .into());
        }
        return Ok(base);
    };
    let Some(target) = repo.rev_parse(&format!("{rev}^{{commit}}")) else {
        return Err(UsageError::new(format!("no commit `{rev}` for --base"))
            .with_hint(format!(
                "git fetch {remote} <branch>, then pass --base {remote}/<branch>"
            ))
            .into());
    };
    match repo.try_output(&["merge-base", &r.local_oid, &target]) {
        Some(b) if !b.is_empty() => Ok(Some(b)),
        _ => {
            let why = if shallow { "shallow clone: " } else { "" };
            Err(
                UsageError::new(format!("{why}HEAD has no merge base with `{rev}`"))
                    .with_hint(DEEPEN)
                    .into(),
            )
        }
    }
}

fn push(
    repo: &Repo,
    base: Option<String>,
    r: &PushRef,
    label: bool,
    report: &mut Report,
) -> Result<()> {
    let tip = &r.local_oid;
    let old = match &base {
        Some(b) => b.clone(),
        None => repo.empty_tree()?,
    };
    let changes = changes::between(repo, &old, tip)?;
    progress::line(format!(
        "checking push of {} ({} changed {} since {})",
        r.local_ref
            .strip_prefix("refs/heads/")
            .unwrap_or(&r.local_ref),
        changes.len(),
        plural(changes.len(), "file"),
        base.as_deref()
            .map_or("the empty tree", |b| &b[..b.len().min(10)]),
    ));
    let current = Snapshot::tree(repo, tip)?;
    let parent = Snapshot::at(repo, base.as_deref())?;
    let before = report.findings.len();
    let t = Instant::now();
    let range = guard::loosening(repo, &parent, &current, &changes)?;
    report.extend(guard::check(&range));
    report.timing("guard", t.elapsed());
    check_tree(
        repo,
        Stage::Push,
        Source::Tree(tip.clone()),
        &current,
        &parent,
        &changes,
        report,
    )?;
    if label {
        let name = r.local_ref.clone();
        for f in &mut report.findings[before..] {
            f.message = format!("{name}: {}", f.message);
        }
    }
    Ok(())
}

/// Built-in checks and repo tools on one tree, in parallel.
fn check_tree(
    repo: &Repo,
    stage: Stage,
    source: Source,
    current: &Snapshot,
    parent: &Snapshot,
    changes: &[Change],
    report: &mut Report,
) -> Result<()> {
    let config = config::load(repo, current)?;
    report.extend(crate::skill::check(repo, current)?);
    let baseline = baseline::from_snapshot(repo, current)?;
    let changed: Vec<String> = changes
        .iter()
        .filter(|c| !c.is_deleted())
        .map(|c| c.path.clone())
        .collect();
    progress::started("built-in checks");
    let status = progress::status_line();
    let (builtin, tools) = std::thread::scope(|s| {
        let builtin = s.spawn(|| {
            let t = Instant::now();
            let r = compare::check(&compare::CheckInput {
                repo,
                config: &config,
                current,
                parent,
                changes,
                baseline: &baseline,
            });
            let errors = r.as_ref().map_or(0, |f| {
                f.iter().filter(|f| f.severity == Severity::Error).count()
            });
            let problem = (errors > 0).then(|| format!("{errors} {}", plural(errors, "error")));
            progress::done("built-in checks", problem, t.elapsed());
            (r, t.elapsed())
        });
        let tools = s.spawn(|| {
            tools::run_tools(&ToolContext {
                repo,
                config: &config,
                stage,
                source,
                changed,
            })
        });
        (
            builtin.join().expect("built-in checks"),
            tools.join().expect("tool runner"),
        )
    });
    drop(status);
    let (findings, elapsed) = builtin;
    report.extend(crate::rules::apply_levels(&config, findings?));
    report.timing("builtin", elapsed);
    let tools = tools?;
    report.extend(tools.findings);
    for (name, d) in tools.timings {
        report.timing(&name, d);
    }
    Ok(())
}
