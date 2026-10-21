//! `qgate install`: git hooks, the baseline merge driver, a starter config,
//! the Claude Code agent hook and the agent skill. Safe to repeat.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::baseline::BASELINE_FILE;
use crate::cli::{Finding, Report, UsageError};
use crate::config::{CONFIG_FILE, Config};
use crate::git::Repo;
use crate::tools::shell_quote;

const MARKER: &str = "# qgate-managed hook";
const CHAINED_SUFFIX: &str = ".pre-qgate";
const ATTRIBUTE_LINE: &str = "qgate.baseline merge=qgate-baseline";
const AGENT_MATCHER: &str = "Bash|Write|Edit|MultiEdit|NotebookEdit";
/// Machine-local: the hook command holds this machine's absolute binary path.
const CLAUDE_SETTINGS: &str = ".claude/settings.local.json";

/// One file install would create or change.
struct Write {
    path: PathBuf,
    content: String,
    executable: bool,
}

pub fn install(repo: &Repo, chain: bool, dry_run: bool) -> Result<Report> {
    let exe = std::env::current_exe()
        .context("locating the qgate binary")?
        .canonicalize()?;
    let exe_q = shell_quote(&exe.to_string_lossy());
    let hooks_dir = hooks_dir(repo, chain)?;

    let mut writes = Vec::new();
    let mut renames = Vec::new();
    for (name, script) in [
        ("pre-commit", commit_hook(&exe_q)),
        ("pre-push", push_hook(&exe_q)),
    ] {
        let path = hooks_dir.join(name);
        let existing = std::fs::read_to_string(&path).ok();
        if let Some(text) = existing.filter(|t| !t.contains(MARKER)) {
            if !chain {
                return Err(UsageError::new(format!(
                    "{} already exists and is not qgate's{}",
                    path.display(),
                    if text.contains("husky") {
                        " (husky)"
                    } else {
                        ""
                    }
                ))
                .with_hint("qgate install --chain   (keeps it and runs it after qgate)")
                .into());
            }
            renames.push((
                path.clone(),
                path.with_file_name(format!("{name}{CHAINED_SUFFIX}")),
            ));
        }
        writes.push(Write {
            path,
            content: script,
            executable: true,
        });
    }

    let attrs = repo.root.join(".gitattributes");
    let attr_text = std::fs::read_to_string(&attrs).unwrap_or_default();
    if !attr_text.lines().any(|l| l.trim() == ATTRIBUTE_LINE) {
        let sep = if attr_text.is_empty() || attr_text.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        writes.push(Write {
            path: attrs,
            content: format!("{attr_text}{sep}{ATTRIBUTE_LINE}\n"),
            executable: false,
        });
    }
    let config = repo.root.join(CONFIG_FILE);
    if !config.exists() {
        writes.push(Write {
            path: config,
            content: Config::starter_for(&repo.root).to_toml(),
            executable: false,
        });
    }
    let skill = crate::skill::render();
    for path in crate::skill::PATHS {
        writes.push(Write {
            path: repo.root.join(path),
            content: skill.clone(),
            executable: false,
        });
    }
    let settings = repo.root.join(CLAUDE_SETTINGS);
    let agent_cmd = format!("{exe_q} agent-hook claude || exit 2");
    if let Some(content) = claude_settings(&settings, &agent_cmd)? {
        writes.push(Write {
            path: settings,
            content,
            executable: false,
        });
    }

    let driver = format!("{exe_q} baseline merge %O %A %B");
    let driver_set = repo
        .try_output(&["config", "--get", "merge.qgate-baseline.driver"])
        .as_deref()
        == Some(driver.as_str());

    let mut changed = Vec::new();
    for w in &writes {
        if std::fs::read_to_string(&w.path).is_ok_and(|t| t == w.content) {
            continue;
        }
        changed.push(rel(repo, &w.path));
        if !dry_run {
            if let Some(dir) = w.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            if let Some((from, to)) = renames.iter().find(|(from, _)| *from == w.path) {
                std::fs::rename(from, to)?;
            }
            std::fs::write(&w.path, &w.content)?;
            if w.executable {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&w.path, std::fs::Permissions::from_mode(0o755))?;
            }
        }
    }
    if !driver_set {
        changed.push("git config merge.qgate-baseline.driver".into());
        if !dry_run {
            repo.output(&[
                "config",
                "merge.qgate-baseline.name",
                "qgate baseline (lower value wins)",
            ])?;
            repo.output(&["config", "merge.qgate-baseline.driver", &driver])?;
        }
    }

    let mut report = Report::new("install");
    report.set("binary", exe.to_string_lossy().to_string());
    report.set("hooks", rel(repo, &hooks_dir));
    report.set(
        if dry_run { "would_change" } else { "changed" },
        json!(changed),
    );
    if !dry_run && !Path::new(&repo.root.join(BASELINE_FILE)).exists() {
        report.extend([
            Finding::info("next-step", "record existing violations")
                .with_fix("qgate baseline create"),
            Finding::info("next-step", "stage the gate's files").with_fix(format!(
                "git add {CONFIG_FILE} {BASELINE_FILE} .gitattributes {}",
                crate::skill::PATHS.join(" ")
            )),
        ]);
    }
    Ok(report.finish())
}

fn rel(repo: &Repo, p: &Path) -> String {
    p.strip_prefix(&repo.root)
        .unwrap_or(p)
        .to_string_lossy()
        .to_string()
}

/// `.git/hooks`, or the configured `core.hooksPath` when chaining onto it.
fn hooks_dir(repo: &Repo, chain: bool) -> Result<PathBuf> {
    if let Some(custom) = repo
        .try_output(&["config", "--get", "core.hooksPath"])
        .filter(|s| !s.is_empty())
    {
        if !chain {
            return Err(UsageError::new(format!(
                "core.hooksPath is set to `{custom}`, so another hook manager owns the hooks"
            ))
            .with_hint("qgate install --chain   (installs into that directory and keeps its hooks)")
            .into());
        }
        let p = PathBuf::from(&custom);
        return Ok(if p.is_absolute() {
            p
        } else {
            repo.root.join(p)
        });
    }
    repo.hooks_dir()
}

fn chained(var: &str) -> String {
    format!(
        "chained=\"$0{CHAINED_SUFFIX}\"\nif [ -x \"$chained\" ]; then\n  {var}exec \"$chained\" \"$@\"\nfi\n"
    )
}

fn commit_hook(exe: &str) -> String {
    format!(
        "#!/bin/sh\n{MARKER}. Reinstall with: qgate install\n{exe} run --stage commit || exit $?\n{}",
        chained("")
    )
}

fn push_hook(exe: &str) -> String {
    format!(
        "#!/bin/sh\n{MARKER}. Reinstall with: qgate install\ninput=$(cat)\nprintf '%s\\n' \"$input\" | {exe} run --stage push --hook-input --remote \"$1\" || exit $?\n{}",
        chained("printf '%s\\n' \"$input\" | ")
    )
}

/// The settings file with the agent hook registered, or `None` when it already is.
fn claude_settings(path: &Path, command: &str) -> Result<Option<String>> {
    let mut root: Value = match std::fs::read_to_string(path) {
        Ok(t) => serde_json::from_str(&t)
            .with_context(|| format!("{} is not valid JSON", path.display()))?,
        Err(_) => json!({}),
    };
    let entry = json!({
        "matcher": AGENT_MATCHER,
        "hooks": [{"type": "command", "command": command}],
    });
    let obj = root
        .as_object_mut()
        .context("settings must be a JSON object")?;
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let pre = hooks
        .as_object_mut()
        .context("`hooks` must be an object")?
        .entry("PreToolUse")
        .or_insert_with(|| json!([]));
    let list = pre
        .as_array_mut()
        .context("`hooks.PreToolUse` must be an array")?;
    let ours = |v: &Value| v.to_string().contains("agent-hook claude");
    if list.iter().filter(|v| ours(v)).count() == 1 && list.contains(&entry) {
        return Ok(None);
    }
    list.retain(|v| !ours(v));
    list.push(entry);
    Ok(Some(serde_json::to_string_pretty(&root)? + "\n"))
}
