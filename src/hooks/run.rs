//! `qgate run`: the whole pipeline for a commit or a push, ending in one result.

use std::io::Read;
use std::time::Instant;

use anyhow::Result;

use crate::baseline::{self, compare};
use crate::cli::{Finding, Report, UsageError};
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
    let current = Snapshot::index(repo)?;
    let parent = Snapshot::at(repo, repo.head().as_deref())?;
    // The guard decides before any repo tool runs, so nothing a tool writes
    // during the run (such as an approval record) can affect it.
    let t = Instant::now();
    let l = guard::loosening(repo, &parent, &current, &changes)?;
    report.extend(guard::check(repo, &l, "qgate approve"));
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
    let current = Snapshot::tree(repo, tip)?;
    let parent = Snapshot::at(repo, base.as_deref())?;
    let before = report.findings.len();
    let t = Instant::now();
    for (commit, first_parent) in changes::commits(repo, base.as_deref(), tip)? {
        let parent = Snapshot::at(repo, first_parent.as_deref())?;
        let current = Snapshot::tree(repo, &commit)?;
        let old = match &first_parent {
            Some(p) => p.clone(),
            None => repo.empty_tree()?,
        };
        let diff = changes::between(repo, &old, &commit)?;
        let l = guard::loosening(repo, &parent, &current, &diff)?;
        let short = &commit[..commit.len().min(10)];
        report.extend(
            guard::check(repo, &l, &format!("qgate approve --commit {short}"))
                .into_iter()
                .map(|f| Finding {
                    message: format!("commit {short}: {}", f.message),
                    ..f
                }),
        );
    }
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
    // Tools from a config with an unapproved loosening may be the loosening
    // itself (a new tool can run anything), so they do not run until approved.
    let approved = !report.findings.iter().any(|f| f.rule == guard::RULE);
    if !approved && !config.tools.is_empty() {
        report.extend([Finding::warning(
            "tools-skipped",
            "repo tools did not run because the change has an unapproved loosening",
        )]);
    }
    let baseline = baseline::from_snapshot(repo, current)?;
    let changed: Vec<String> = changes
        .iter()
        .filter(|c| !c.is_deleted())
        .map(|c| c.path.clone())
        .collect();
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
            (r, t.elapsed())
        });
        let tools = s.spawn(|| {
            if !approved {
                return Ok(tools::ToolResults::default());
            }
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
    let (findings, elapsed) = builtin;
    report.extend(findings?);
    report.timing("builtin", elapsed);
    let tools = tools?;
    report.extend(tools.findings);
    for (name, d) in tools.timings {
        report.timing(&name, d);
    }
    Ok(())
}
