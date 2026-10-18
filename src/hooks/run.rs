//! `qgate run`: the whole pipeline for a commit or a push, ending in one result.

use std::io::Read;
use std::time::Instant;

use anyhow::Result;

use crate::baseline::{self, compare};
use crate::cli::{Finding, Report};
use crate::config::{self, Stage};
use crate::git::{Change, PushRef, Repo, Snapshot, Source, ZERO_OID, changes};
use crate::guard;
use crate::tools::{self, ToolContext};

pub fn run(repo: &Repo, stage: Stage, remote: &str, hook_input: bool) -> Result<Report> {
    let start = Instant::now();
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
                push(repo, remote, r, refs.len() > 1, &mut report)?;
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
    check_tree(
        repo,
        Stage::Commit,
        Source::Index,
        &current,
        &parent,
        &changes,
        report,
    )?;
    let t = Instant::now();
    let l = guard::loosening(repo, &parent, &current, &changes)?;
    report.extend(guard::check(repo, &l, "qgate approve"));
    report.timing("guard", t.elapsed());
    Ok(())
}

fn push(repo: &Repo, remote: &str, r: &PushRef, label: bool, report: &mut Report) -> Result<()> {
    let tip = &r.local_oid;
    let base = changes::push_base(repo, remote, r);
    let old = match &base {
        Some(b) => b.clone(),
        None => repo.empty_tree()?,
    };
    let changes = changes::between(repo, &old, tip)?;
    let current = Snapshot::tree(repo, tip)?;
    let parent = Snapshot::at(repo, base.as_deref())?;
    let before = report.findings.len();
    check_tree(
        repo,
        Stage::Push,
        Source::Tree(tip.clone()),
        &current,
        &parent,
        &changes,
        report,
    )?;
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
