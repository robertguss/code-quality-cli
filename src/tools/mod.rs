//! Run the repo's own linters, type checkers and tests (declared as
//! `[[tool]]` in `qgate.toml`) against exactly the content being checked.

pub mod parsers;
pub mod scheduler;

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::cli::output::{Finding, Severity};
use crate::config::{Config, Stage, Tool};
use crate::git::checkout::DEPENDENCY_DIRS;
use crate::git::{Checkout, Repo, Source, changes};
use parsers::Level;
use scheduler::{Exit, Job, Outcome};

/// Output lines kept in a pass-through finding; the full output is in the log.
const SNIPPET_LINES: usize = 40;
const SNIPPET_LINE_CHARS: usize = 300;

pub struct ToolContext<'a> {
    pub repo: &'a Repo,
    pub config: &'a Config,
    pub stage: Stage,
    /// What is being checked: `Source::Index` at commit, `Source::Tree(tip)` at push.
    pub source: Source,
    /// Repo-relative paths of added, modified or renamed (not deleted) files.
    pub changed: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ToolResults {
    pub findings: Vec<Finding>,
    /// `("tool:<name>", wall time)` per tool that ran, in config order.
    pub timings: Vec<(String, Duration)>,
}

/// A tool chosen to run, with its command expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned<'a> {
    pub tool: &'a Tool,
    /// The shell command with `{files}` replaced; also the re-run command.
    pub command: String,
    /// Changed files matching the tool's globs.
    pub files: Vec<String>,
    /// Whether it must run in a temporary checkout rather than the working tree.
    pub in_checkout: bool,
}

/// Decide which tools run, on which files, and where (KTD3, KTD9).
pub fn plan<'a>(ctx: &ToolContext<'a>) -> Result<Vec<Planned<'a>>> {
    let mut chosen = Vec::new();
    for tool in &ctx.config.tools {
        if ctx.stage == Stage::Commit && tool.stage == Stage::Push {
            continue;
        }
        let globs = glob_set(tool)?;
        let files: Vec<String> = ctx
            .changed
            .iter()
            .filter(|p| matches(&globs, p))
            .cloned()
            .collect();
        // At push a whole-project tool always runs; otherwise it needs a match.
        let whole_project_push = ctx.stage == Stage::Push && !tool.takes_files();
        if files.is_empty() && !whole_project_push {
            continue;
        }
        chosen.push((tool, globs, files));
    }
    let dirty = match (&ctx.source, chosen.is_empty()) {
        (Source::Index, false) => Some((
            changes::unstaged_paths(ctx.repo)?,
            changes::untracked_paths(ctx.repo)?,
        )),
        _ => None,
    };
    Ok(chosen
        .into_iter()
        .map(|(tool, globs, files)| {
            let in_checkout = match &dirty {
                None => true,
                Some((unstaged, untracked)) => needs_checkout(tool, &globs, unstaged, untracked),
            };
            let quoted: Vec<String> = files.iter().map(|f| shell_quote(f)).collect();
            Planned {
                command: tool.command.replace("{files}", &quoted.join(" ")),
                tool,
                files,
                in_checkout,
            }
        })
        .collect())
}

/// Run the planned tools in parallel and turn their results into findings.
pub fn run_tools(ctx: &ToolContext) -> Result<ToolResults> {
    let planned = plan(ctx)?;
    let mut results = ToolResults::default();
    if planned.is_empty() {
        return Ok(results);
    }
    let package_dirs: Vec<String> = ctx
        .config
        .packages
        .iter()
        .filter_map(|p| p.path.clone())
        .collect();
    let checkout = match planned.iter().any(|p| p.in_checkout) {
        true => Some(Checkout::create(ctx.repo, &ctx.source, &package_dirs)?),
        false => None,
    };
    let jobs: Vec<Job> = planned
        .iter()
        .map(|p| Job {
            command: p.command.clone(),
            cwd: match (&checkout, p.in_checkout) {
                (Some(c), true) => c.root().to_path_buf(),
                _ => ctx.repo.root.clone(),
            },
            timeout: Duration::from_secs(p.tool.timeout_for(ctx.stage)),
        })
        .collect();
    let outcomes = scheduler::parallel_map(&jobs, scheduler::run);

    let logs = ctx.repo.state_dir().join("logs");
    std::fs::create_dir_all(&logs).with_context(|| format!("creating {}", logs.display()))?;
    let mut roots: Vec<String> = Vec::new();
    if let Some(c) = &checkout {
        roots.push(c.root().display().to_string());
        if let Ok(canon) = c.root().canonicalize() {
            roots.push(canon.display().to_string());
        }
    }
    roots.push(ctx.repo.root.display().to_string());

    for ((p, job), outcome) in planned.iter().zip(&jobs).zip(&outcomes) {
        let log = logs.join(format!("{}.log", file_name(&p.tool.name)));
        std::fs::write(&log, log_text(job, outcome))
            .with_context(|| format!("writing {}", log.display()))?;
        let log_shown = log
            .strip_prefix(&ctx.repo.root)
            .unwrap_or(&log)
            .display()
            .to_string();
        results
            .findings
            .extend(interpret(ctx.stage, p, outcome, &roots, &log_shown));
        results
            .timings
            .push((format!("tool:{}", p.tool.name), outcome.elapsed));
    }
    Ok(results)
}

fn glob_set(tool: &Tool) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for g in &tool.globs {
        b.add(Glob::new(g).with_context(|| format!("tool `{}`: bad glob `{g}`", tool.name))?);
    }
    Ok(b.build()?)
}

/// Empty globs match every file.
fn matches(globs: &GlobSet, path: &str) -> bool {
    globs.is_empty() || globs.is_match(path)
}

/// KTD3: the working tree is only safe when it holds exactly the staged
/// content the tool could read. A whole-project tool may read anything.
fn needs_checkout(
    tool: &Tool,
    globs: &GlobSet,
    unstaged: &BTreeSet<String>,
    untracked: &BTreeSet<String>,
) -> bool {
    if !tool.takes_files() {
        return !unstaged.is_empty() || !untracked.is_empty();
    }
    unstaged.iter().chain(untracked).any(|p| matches(globs, p))
}

/// Quote a path for `sh`, leaving plain paths readable.
pub fn shell_quote(s: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_-./+@%:,=".contains(c);
    if !s.is_empty() && s.chars().all(plain) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// The program a command runs, skipping leading `VAR=value` assignments.
fn program(command: &str) -> &str {
    command
        .split_whitespace()
        .find(|w| {
            !w.split_once('=').is_some_and(|(k, _)| {
                !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
        })
        .unwrap_or(command)
}

fn file_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn log_text(job: &Job, o: &Outcome) -> Vec<u8> {
    let mut text = format!(
        "$ {}\n# cwd: {}\n# result: {:?} after {:.1}s\n",
        job.command,
        job.cwd.display(),
        o.exit,
        o.elapsed.as_secs_f64()
    )
    .into_bytes();
    text.extend_from_slice(&o.stdout);
    if !o.stdout.is_empty() && !o.stdout.ends_with(b"\n") {
        text.push(b'\n');
    }
    text.extend_from_slice(&o.stderr);
    text
}

/// Make a path from tool output repo-relative.
fn relativize(path: &str, roots: &[String]) -> String {
    for root in roots {
        if let Some(rest) = path
            .strip_prefix(root.as_str())
            .and_then(|r| r.strip_prefix('/'))
        {
            return rest.to_string();
        }
    }
    path.strip_prefix("./").unwrap_or(path).to_string()
}

/// Strip temp-checkout and repo prefixes from free text.
fn scrub(text: &str, roots: &[String]) -> String {
    roots.iter().fold(text.to_string(), |t, root| {
        t.replace(&format!("{root}/"), "")
    })
}

/// At most [`SNIPPET_LINES`] lines: the head and tail, with a gap marker.
fn snippet(text: &str) -> String {
    let lines: Vec<String> = text
        .trim_end()
        .lines()
        .map(|l| match l.char_indices().nth(SNIPPET_LINE_CHARS) {
            Some((i, _)) => format!("{}...", &l[..i]),
            None => l.to_string(),
        })
        .collect();
    if lines.len() <= SNIPPET_LINES {
        return lines.join("\n");
    }
    let half = SNIPPET_LINES / 2;
    format!(
        "{}\n... ({} lines omitted) ...\n{}",
        lines[..half].join("\n"),
        lines.len() - 2 * half,
        lines[lines.len() - half..].join("\n")
    )
}

fn interpret(stage: Stage, p: &Planned, o: &Outcome, roots: &[String], log: &str) -> Vec<Finding> {
    let name = &p.tool.name;
    let rerun = p.command.clone();
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stderr = String::from_utf8_lossy(&o.stderr);
    let output = || {
        let text = scrub(&format!("{stdout}{stderr}"), roots);
        match text.trim().is_empty() {
            true => String::new(),
            false => format!(":\n{}", snippet(&text)),
        }
    };
    let code = match &o.exit {
        Exit::SpawnFailed(e) => {
            return vec![
                Finding::error("tool-error", format!("`{name}` could not be run: {e}"))
                    .with_fix(rerun),
            ];
        }
        Exit::Signal(sig) => {
            return vec![
                Finding::error(
                    "tool-error",
                    format!(
                        "`{name}` was killed by signal {sig}{}\nfull output: {log}",
                        output()
                    ),
                )
                .with_fix(rerun),
            ];
        }
        Exit::TimedOut => {
            let secs = p.tool.timeout_for(stage);
            return vec![
                Finding::error(
                    "tool-timeout",
                    format!(
                        "`{name}` did not finish within {secs}s and was stopped\nfull output: {log}"
                    ),
                )
                .with_fix(format!(
                    "{rerun} (if it is only slow, raise tool.{name}.timeout_secs in qgate.toml)"
                )),
            ];
        }
        Exit::Code(127) => {
            let prog = program(&p.tool.command);
            return vec![
                Finding::error(
                    "tool-missing",
                    format!(
                        "`{name}`: command not found running `{}`{}",
                        p.tool.command,
                        output()
                    ),
                )
                .with_fix(format!(
                    "install {prog}, or fix tool.{name}.command in qgate.toml"
                )),
            ];
        }
        Exit::Code(code) => *code,
    };

    let parsed = parsers::parse(p.tool.format, &stdout, &stderr);
    let failed = || {
        let out = output();
        let message = match out.is_empty() {
            true => format!(
                "`{name}` failed (exit {code}) and printed nothing to say why; if it \
                 signals only by exit code, as `test -z` does, make the command print \
                 what failed first"
            ),
            false => format!("`{name}` failed (exit {code}){out}\nfull output: {log}"),
        };
        Finding::error(name, message).with_fix(rerun.clone())
    };
    if parsed.is_empty() {
        return match code {
            0 => Vec::new(),
            _ => vec![failed()],
        };
    }
    // A finding is an error only when the tool failed and it is the tool's own
    // error in the repo's code. Dependency and out-of-repo findings only warn.
    let mut findings: Vec<Finding> = parsed
        .into_iter()
        .map(|f| {
            let rule = match &f.rule {
                Some(r) => format!("{name}/{r}"),
                None => name.clone(),
            };
            let message = scrub(&f.message, roots);
            let path = f.path.as_deref().map(|p| relativize(p, roots));
            let own = path.as_deref().is_none_or(is_own_path);
            let finding = match code != 0 && f.level == Level::Error && own {
                true => Finding::error(&rule, message).with_fix(rerun.clone()),
                false => Finding::warning(&rule, message),
            };
            match path {
                Some(path) => finding.at(path, f.line),
                None => finding,
            }
        })
        .collect();
    // A failing exit always fails the run, even when nothing parsed explains
    // it: the real failure may be in output no parser recognised.
    if code != 0 && findings.iter().all(|f| f.severity != Severity::Error) {
        findings.insert(0, failed());
    }
    findings
}

/// Whether a repo-relative path from tool output is the repo's own code,
/// rather than outside the repo or inside a dependency or build directory.
fn is_own_path(path: &str) -> bool {
    if path.starts_with('/') || path.starts_with("../") {
        return false;
    }
    let dirs: Vec<&str> = path.split('/').collect();
    let dirs = &dirs[..dirs.len().saturating_sub(1)];
    !DEPENDENCY_DIRS.iter().any(|dep| {
        let dep: Vec<&str> = dep.split('/').collect();
        dirs.windows(dep.len()).any(|w| w == dep.as_slice())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_and_program_names() {
        assert_eq!(shell_quote("src/a.ts"), "src/a.ts");
        assert_eq!(shell_quote("it's here.ts"), r"'it'\''s here.ts'");
        assert_eq!(program("FOO=1 BAR=x eslint --fix ."), "eslint");
        assert_eq!(program("npx tsc"), "npx");
    }

    #[test]
    fn paths_are_made_repo_relative() {
        let roots = vec!["/tmp/qgate-checkout-x".to_string(), "/repo".to_string()];
        assert_eq!(
            relativize("/tmp/qgate-checkout-x/src/a.ts", &roots),
            "src/a.ts"
        );
        assert_eq!(relativize("/repo/b.py", &roots), "b.py");
        assert_eq!(relativize("./c.go", &roots), "c.go");
        assert_eq!(relativize("/usr/include/x.h", &roots), "/usr/include/x.h");
    }

    #[test]
    fn dependency_and_outside_paths_are_not_own_code() {
        assert!(is_own_path("lib/elara/x.ex"));
        assert!(is_own_path("deps.ex"));
        assert!(!is_own_path("deps/yamerl/src/yamerl.erl"));
        assert!(!is_own_path("apps/web/node_modules/x/index.js"));
        assert!(!is_own_path("lab/_build/dev/lib/x.ex"));
        assert!(!is_own_path("app/vendor/bundle/gems/x.rb"));
        assert!(is_own_path("vendor/x.rb"));
        assert!(!is_own_path("/usr/include/stdio.h"));
        assert!(!is_own_path("../other/a.c"));
    }

    #[test]
    fn snippet_keeps_head_and_tail() {
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        let s = snippet(&text);
        assert_eq!(s.lines().count(), SNIPPET_LINES + 1);
        assert!(s.contains("line 1\n") && s.contains("line 100") && s.contains("60 lines omitted"));
    }
}
