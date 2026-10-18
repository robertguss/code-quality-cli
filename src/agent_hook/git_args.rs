//! Git argument parsing with per-subcommand flag tables, so `git log -n 5`
//! passes while `git commit -nm x` does not. Aliases are resolved first.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::paths::Access;
use super::{Arg, reason};

/// What a git invocation needs checked, beyond its own flags.
#[derive(Debug, PartialEq)]
pub enum Effect {
    Block(String),
    /// A shell string git will run (`!` aliases, `rebase --exec`).
    Shell {
        script: String,
        dir: PathBuf,
    },
    /// Pathspecs that must not name protected paths.
    Paths {
        paths: Vec<String>,
        dir: PathBuf,
        access: Access,
    },
    /// Arguments to scan for protected-path fragments.
    Scan(Vec<String>),
}

/// Looks up `alias.<name>` in the repository at a directory, with the git
/// global options (`--git-dir`, `--work-tree`) of the invocation.
pub type AliasLookup<'a> = &'a dyn Fn(&Path, &[String], &str) -> Option<String>;

const MAX_ALIAS_DEPTH: usize = 8;

/// Commands git runs itself; an alias of the same name is never used.
const BUILTINS: &[&str] = &[
    "add",
    "am",
    "annotate",
    "apply",
    "archive",
    "backfill",
    "bisect",
    "blame",
    "branch",
    "bugreport",
    "bundle",
    "cat-file",
    "check-attr",
    "check-ignore",
    "check-mailmap",
    "check-ref-format",
    "checkout",
    "checkout-index",
    "cherry",
    "cherry-pick",
    "citool",
    "clean",
    "clone",
    "column",
    "commit",
    "commit-graph",
    "commit-tree",
    "config",
    "count-objects",
    "credential",
    "describe",
    "diagnose",
    "diff",
    "diff-files",
    "diff-index",
    "diff-tree",
    "difftool",
    "fast-export",
    "fast-import",
    "fetch",
    "fetch-pack",
    "filter-branch",
    "for-each-ref",
    "for-each-repo",
    "format-patch",
    "fsck",
    "gc",
    "get-tar-commit-id",
    "grep",
    "gui",
    "hash-object",
    "help",
    "hook",
    "index-pack",
    "init",
    "init-db",
    "instaweb",
    "interpret-trailers",
    "log",
    "ls-files",
    "ls-remote",
    "ls-tree",
    "mailinfo",
    "mailsplit",
    "maintenance",
    "merge",
    "merge-base",
    "merge-file",
    "merge-index",
    "merge-tree",
    "mergetool",
    "mktag",
    "mktree",
    "multi-pack-index",
    "mv",
    "name-rev",
    "notes",
    "pack-objects",
    "pack-refs",
    "patch-id",
    "prune",
    "prune-packed",
    "pull",
    "push",
    "range-diff",
    "read-tree",
    "rebase",
    "receive-pack",
    "reflog",
    "refs",
    "remote",
    "repack",
    "replace",
    "replay",
    "request-pull",
    "rerere",
    "reset",
    "restore",
    "rev-list",
    "rev-parse",
    "revert",
    "rm",
    "send-email",
    "send-pack",
    "shortlog",
    "show",
    "show-branch",
    "show-index",
    "show-ref",
    "sparse-checkout",
    "stash",
    "status",
    "stripspace",
    "submodule",
    "switch",
    "symbolic-ref",
    "tag",
    "unpack-objects",
    "update-index",
    "update-ref",
    "update-server-info",
    "upload-pack",
    "var",
    "verify-commit",
    "verify-pack",
    "verify-tag",
    "version",
    "whatchanged",
    "worktree",
    "write-tree",
];

/// Subcommands whose flags or messages are already fully classified, or that
/// only read; everything else has its arguments scanned for protected paths.
const CLASSIFIED: &[&str] = &[
    "add",
    "am",
    "annotate",
    "bisect",
    "blame",
    "branch",
    "cat-file",
    "check-attr",
    "check-ignore",
    "checkout",
    "cherry",
    "cherry-pick",
    "clean",
    "clone",
    "commit",
    "config",
    "count-objects",
    "describe",
    "diff",
    "diff-files",
    "diff-index",
    "diff-tree",
    "fetch",
    "for-each-ref",
    "grep",
    "help",
    "log",
    "ls-files",
    "ls-remote",
    "ls-tree",
    "merge",
    "merge-base",
    "name-rev",
    "notes",
    "pull",
    "push",
    "range-diff",
    "rebase",
    "reflog",
    "remote",
    "reset",
    "restore",
    "rev-list",
    "rev-parse",
    "revert",
    "rm",
    "shortlog",
    "show",
    "show-branch",
    "show-ref",
    "stash",
    "status",
    "submodule",
    "switch",
    "tag",
    "var",
    "verify-commit",
    "verify-tag",
    "version",
    "whatchanged",
    "worktree",
];

/// Git global options that take the next argument as their value.
const GLOBAL_WITH_VALUE: &[&str] = &[
    "-C",
    "-c",
    "--config-env",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--super-prefix",
    "--attr-source",
    "--list-cmds",
];

/// Classify `git <args>` run from `cwd`.
pub fn analyze(args: &[Arg], cwd: &Path, lookup: AliasLookup) -> Vec<Effect> {
    let mut out = Vec::new();
    Analyzer {
        lookup,
        out: &mut out,
    }
    .git(args, cwd, &HashMap::new(), 0);
    out
}

/// Classify the value of `alias.<name>` as if it were invoked.
pub fn analyze_alias(value: &str, cwd: &Path, lookup: AliasLookup) -> Vec<Effect> {
    let mut out = Vec::new();
    Analyzer {
        lookup,
        out: &mut out,
    }
    .alias_value(value, &[], cwd, &HashMap::new(), 0);
    out
}

struct Analyzer<'a, 'o> {
    lookup: AliasLookup<'a>,
    out: &'o mut Vec<Effect>,
}

fn lit(s: &str) -> Arg {
    Arg {
        text: s.to_string(),
        raw: s.to_string(),
        dynamic: false,
    }
}

/// Whether a long option name (without `--` or `=value`) abbreviates `--no-verify`.
fn is_no_verify(name: &str) -> bool {
    name.len() >= 4 && "no-verify".starts_with(name)
}

/// A config key in canonical case: section and name lowercased, subsection kept.
fn canonical_key(key: &str) -> String {
    match (key.find('.'), key.rfind('.')) {
        (Some(first), Some(last)) if first != last => format!(
            "{}{}{}",
            key[..first].to_lowercase(),
            &key[first..last],
            key[last..].to_lowercase()
        ),
        _ => key.to_lowercase(),
    }
}

/// Subcommands that run hooks, where an unknowable argument could skip them.
const HOOK_RUNNERS: &[&str] = &[
    "push",
    "merge",
    "am",
    "rebase",
    "pull",
    "cherry-pick",
    "revert",
];

/// Config keys that can disable or redirect hooks.
fn is_hook_key(key: &str) -> bool {
    let k = canonical_key(key);
    k == "core.hookspath"
        || k == "core.attributesfile"
        || k == "include.path"
        || (k.starts_with("includeif.") && k.ends_with(".path"))
}

fn alias_name(key: &str) -> Option<String> {
    let (section, name) = key.split_once('.')?;
    section
        .eq_ignore_ascii_case("alias")
        .then(|| name.to_lowercase())
}

/// Split an alias value the way git does: whitespace, quotes and backslashes.
fn split_cmdline(s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (Some(q), c) if c == q => quote = None,
            (q, '\\') if q != Some('\'') => {
                cur.push(chars.next()?);
                in_word = true;
            }
            (_, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        out.push(cur);
    }
    Some(out)
}

fn quoted(args: &[Arg]) -> String {
    args.iter()
        .map(|a| super::shell::quote(&a.text))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Analyzer<'_, '_> {
    fn block(&mut self, why: String) {
        self.out.push(Effect::Block(why));
    }

    fn git(
        &mut self,
        args: &[Arg],
        cwd: &Path,
        outer_aliases: &HashMap<String, Option<String>>,
        depth: usize,
    ) {
        let mut dir = cwd.to_path_buf();
        let mut git_opts: Vec<String> = Vec::new();
        let mut aliases = outer_aliases.clone();
        let mut i = 0;
        while i < args.len() {
            let a = args[i].text.as_str();
            let (name, attached) = match a.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
                _ => (a, None),
            };
            if !a.starts_with('-') {
                break;
            }
            let value = if attached.is_some() {
                attached
            } else if GLOBAL_WITH_VALUE.contains(&name) {
                i += 1;
                Some(args.get(i).map(|v| v.text.clone()).unwrap_or_default())
            } else {
                None
            };
            i += 1;
            match (name, value) {
                ("-c", Some(kv)) => {
                    let (key, val) = kv.split_once('=').unwrap_or((&kv, ""));
                    if is_hook_key(key) {
                        self.block(reason::bypass(&format!("-c {key}")));
                    } else if let Some(alias) = alias_name(key) {
                        aliases.insert(alias, Some(val.to_string()));
                    }
                }
                ("--config-env", Some(kv)) => {
                    let key = kv.split_once('=').map_or(kv.as_str(), |(k, _)| k);
                    if is_hook_key(key) {
                        self.block(reason::bypass(&format!("--config-env={key}")));
                    } else if let Some(alias) = alias_name(key) {
                        aliases.insert(alias, None);
                    }
                }
                ("-C", Some(d)) => dir = dir.join(d),
                ("--git-dir" | "--work-tree", Some(v)) => {
                    let p = dir.join(v);
                    git_opts.push(format!("{name}={}", p.display()));
                }
                (
                    "--help" | "-h" | "--version" | "-v" | "--html-path" | "--man-path"
                    | "--info-path",
                    _,
                ) => return,
                _ => {}
            }
        }
        let Some(sub_arg) = args.get(i) else {
            return;
        };
        if sub_arg.dynamic {
            self.block(reason::unverifiable(&format!("git {}", sub_arg.raw)));
            return;
        }
        let sub = sub_arg.text.as_str();
        let rest = &args[i + 1..];
        if !BUILTINS.contains(&sub) {
            let value = match aliases.get(&sub.to_lowercase()) {
                Some(Some(v)) => Some(v.clone()),
                Some(None) => {
                    self.block(reason::unverifiable(&format!("git alias {sub}")));
                    return;
                }
                None => (self.lookup)(&dir, &git_opts, sub),
            };
            if let Some(value) = value {
                self.alias_value(&value, rest, &dir, &aliases, depth);
            }
            return;
        }
        self.subcommand(sub, rest, &dir, &aliases, depth);
    }

    fn alias_value(
        &mut self,
        value: &str,
        rest: &[Arg],
        dir: &Path,
        aliases: &HashMap<String, Option<String>>,
        depth: usize,
    ) {
        if depth >= MAX_ALIAS_DEPTH {
            self.block(reason::unverifiable("a git alias that expands too deeply"));
            return;
        }
        if let Some(script) = value.trim_start().strip_prefix('!') {
            let mut script = script.to_string();
            if !rest.is_empty() {
                script.push(' ');
                script.push_str(&quoted(rest));
            }
            self.out.push(Effect::Shell {
                script,
                dir: dir.to_path_buf(),
            });
            return;
        }
        let Some(words) = split_cmdline(value) else {
            self.block(reason::unverifiable(&format!("git alias `{value}`")));
            return;
        };
        let mut expanded: Vec<Arg> = words.iter().map(|w| lit(w)).collect();
        expanded.extend(rest.iter().cloned());
        self.git(&expanded, dir, aliases, depth + 1);
    }

    fn subcommand(
        &mut self,
        sub: &str,
        rest: &[Arg],
        dir: &Path,
        aliases: &HashMap<String, Option<String>>,
        depth: usize,
    ) {
        match sub {
            "commit-tree" | "update-ref" | "replace" => {
                self.block(reason::history(sub));
                return;
            }
            "commit" => self.commit(rest),
            "config" => self.config(rest, dir, aliases, depth),
            "checkout" | "restore" => self.pathspecs(
                rest,
                dir,
                Access::Write,
                &[
                    "-b",
                    "-B",
                    "--orphan",
                    "-s",
                    "--source",
                    "--conflict",
                    "--pathspec-from-file",
                    "-U",
                    "--unified",
                    "--inter-hunk-context",
                ],
            ),
            "rm" => self.pathspecs(rest, dir, Access::Remove, &["--pathspec-from-file"]),
            "clean" => self.clean(rest, dir),
            "rebase" => self.rebase(rest, dir),
            "submodule" => self.submodule(rest, dir),
            "bisect" if rest.first().is_some_and(|a| a.text == "run") && rest.len() > 1 => {
                self.out.push(Effect::Shell {
                    script: quoted(&rest[1..]),
                    dir: dir.to_path_buf(),
                });
            }
            _ => {}
        }
        if sub != "commit" {
            self.generic_no_verify(sub, rest);
        }
        if !CLASSIFIED.contains(&sub) {
            self.out
                .push(Effect::Scan(rest.iter().map(|a| a.raw.clone()).collect()));
        }
    }

    /// `--no-verify` (or an abbreviation of it) on any subcommand.
    fn generic_no_verify(&mut self, sub: &str, rest: &[Arg]) {
        for a in rest {
            if a.text == "--" {
                break;
            }
            if let Some(long) = a.text.strip_prefix("--") {
                let name = long.split_once('=').map_or(long, |(n, _)| n);
                if is_no_verify(name) {
                    self.block(reason::bypass(&format!("git {sub} {}", a.text)));
                    return;
                }
            } else if a.dynamic && (HOOK_RUNNERS.contains(&sub) || super::mentions_bypass(&a.raw)) {
                self.block(reason::unverifiable(&format!("git {sub} {}", a.raw)));
                return;
            }
        }
    }

    fn commit(&mut self, rest: &[Arg]) {
        const SHORT_VALUE: &str = "mFCct";
        const SHORT_OPTIONAL: &str = "Su";
        const LONG_VALUE: &[&str] = &[
            "message",
            "file",
            "reuse-message",
            "reedit-message",
            "template",
            "author",
            "date",
            "cleanup",
            "fixup",
            "squash",
            "trailer",
            "pathspec-from-file",
        ];
        let mut i = 0;
        while i < rest.len() {
            let a = &rest[i];
            i += 1;
            let t = a.text.as_str();
            if t == "--" {
                break;
            }
            // Option values are skipped above, so a dynamic word here could be any flag.
            if a.dynamic {
                self.block(reason::unverifiable(&format!("git commit {}", a.raw)));
                return;
            }
            if let Some(long) = t.strip_prefix("--") {
                if let Some((name, _)) = long.split_once('=') {
                    if is_no_verify(name) {
                        self.block(reason::bypass(&format!("git commit {t}")));
                    }
                    continue;
                }
                if is_no_verify(long) {
                    self.block(reason::bypass(&format!("git commit {t}")));
                    continue;
                }
                let matches: Vec<_> = LONG_VALUE.iter().filter(|o| o.starts_with(long)).collect();
                if !long.is_empty() && (LONG_VALUE.contains(&long) || matches.len() == 1) {
                    i += 1;
                }
                continue;
            }
            if let Some(cluster) = t.strip_prefix('-').filter(|c| !c.is_empty()) {
                for (k, c) in cluster.char_indices() {
                    if c == 'n' {
                        self.block(reason::bypass(&format!("git commit {t}")));
                        break;
                    }
                    if SHORT_VALUE.contains(c) {
                        if k + 1 == cluster.len() {
                            i += 1;
                        }
                        break;
                    }
                    if SHORT_OPTIONAL.contains(c) {
                        break;
                    }
                }
                continue;
            }
            if a.dynamic && super::mentions_bypass(&a.raw) {
                self.block(reason::unverifiable(&format!("git commit {}", a.raw)));
            }
        }
    }

    fn config(
        &mut self,
        rest: &[Arg],
        dir: &Path,
        aliases: &HashMap<String, Option<String>>,
        depth: usize,
    ) {
        const VALUE_OPTS: &[&str] = &[
            "--file",
            "-f",
            "--blob",
            "--type",
            "--default",
            "--comment",
            "--value",
            "--url",
        ];
        let mut action: Option<&str> = None;
        let mut i = 0;
        if let Some(first) = rest.first().map(|a| a.text.as_str())
            && matches!(
                first,
                "list" | "get" | "set" | "unset" | "rename-section" | "remove-section" | "edit"
            )
        {
            action = Some(first);
            i = 1;
        }
        let mut file: Option<String> = None;
        let mut positional: Vec<&Arg> = Vec::new();
        while i < rest.len() {
            let a = &rest[i];
            i += 1;
            let t = a.text.as_str();
            let (name, attached) = match t.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n, Some(v)),
                _ => (t, None),
            };
            // git stops option parsing at the key, so a value may start with `-`.
            if !t.starts_with('-') || t == "-" || !positional.is_empty() {
                positional.push(a);
                continue;
            }
            if t == "--" {
                positional.extend(&rest[i..]);
                break;
            }
            if VALUE_OPTS.contains(&name) {
                let v = match attached {
                    Some(v) => v.to_string(),
                    None => {
                        i += 1;
                        rest.get(i - 1).map(|v| v.text.clone()).unwrap_or_default()
                    }
                };
                if name == "--file" || name == "-f" {
                    file = Some(v);
                }
                continue;
            }
            match name {
                "--get" | "--get-all" | "--get-regexp" | "--get-urlmatch" | "--list" | "-l"
                | "--unset" | "--unset-all" | "--remove-section" | "--get-color"
                | "--get-colorbool" => action = Some("read"),
                "--add" => action = Some("set"),
                "--replace-all" => action = Some("set"),
                "--rename-section" => action = Some("rename-section"),
                "--edit" | "-e" => action = Some("edit"),
                _ => {}
            }
        }
        let (key, value) = match action {
            None if positional.len() >= 2 => (positional[0], Some(positional[1])),
            Some("set") if !positional.is_empty() => (positional[0], positional.get(1).copied()),
            Some("edit") => {
                self.block(reason::bypass("git config --edit"));
                return;
            }
            Some("rename-section") if positional.len() >= 2 => {
                let to = positional[1].text.to_lowercase();
                if ["core", "alias", "include"]
                    .iter()
                    .any(|s| to.starts_with(s))
                {
                    self.block(reason::bypass(&format!(
                        "git config rename-section {}",
                        positional[1].text
                    )));
                }
                return;
            }
            _ => return,
        };
        if key.dynamic {
            self.block(reason::unverifiable(&format!("git config {}", key.raw)));
            return;
        }
        if let Some(f) = file {
            self.out.push(Effect::Paths {
                paths: vec![f],
                dir: dir.to_path_buf(),
                access: Access::Write,
            });
        }
        if is_hook_key(&key.text) {
            self.block(reason::bypass(&format!("git config {}", key.text)));
        } else if alias_name(&key.text).is_some()
            && let Some(v) = value
        {
            if v.dynamic && super::mentions_bypass(&v.raw) {
                self.block(reason::unverifiable(&format!(
                    "git config {} {}",
                    key.text, v.raw
                )));
                return;
            }
            let before = self.out.len();
            self.alias_value(&v.text, &[], dir, aliases, depth);
            let blocked = self.out[before..]
                .iter()
                .any(|e| matches!(e, Effect::Block(_)));
            if blocked {
                self.out.truncate(before);
                self.block(reason::bypass(&format!(
                    "git config {} '{}'",
                    key.text, v.text
                )));
            }
        }
    }

    fn pathspecs(&mut self, rest: &[Arg], dir: &Path, access: Access, value_opts: &[&str]) {
        let mut paths = Vec::new();
        let mut i = 0;
        let mut after_dashes = false;
        while i < rest.len() {
            let t = rest[i].text.as_str();
            i += 1;
            if after_dashes {
                paths.push(t.to_string());
            } else if t == "--" {
                after_dashes = true;
            } else if t.starts_with('-') {
                if value_opts.contains(&t) {
                    i += 1;
                }
            } else {
                paths.push(t.to_string());
            }
        }
        self.out.push(Effect::Paths {
            paths,
            dir: dir.to_path_buf(),
            access,
        });
    }

    fn clean(&mut self, rest: &[Arg], dir: &Path) {
        let dry = rest.iter().any(|a| {
            a.text == "--dry-run"
                || (a.text.starts_with('-') && !a.text.starts_with("--") && a.text.contains('n'))
        });
        if dry {
            return;
        }
        let mut paths: Vec<String> = rest
            .iter()
            .filter(|a| !a.text.starts_with('-'))
            .map(|a| a.text.clone())
            .collect();
        if paths.is_empty() {
            paths.push(".".into());
        }
        self.out.push(Effect::Paths {
            paths,
            dir: dir.to_path_buf(),
            access: Access::Remove,
        });
    }

    fn rebase(&mut self, rest: &[Arg], dir: &Path) {
        let mut i = 0;
        while i < rest.len() {
            let t = rest[i].text.as_str();
            i += 1;
            let script = if t == "-x" || t == "--exec" {
                i += 1;
                rest.get(i - 1).map(|a| a.text.clone())
            } else {
                t.strip_prefix("--exec=")
                    .or_else(|| t.strip_prefix("-x").filter(|v| !v.is_empty()))
                    .map(str::to_string)
            };
            if let Some(script) = script {
                self.out.push(Effect::Shell {
                    script,
                    dir: dir.to_path_buf(),
                });
            }
        }
    }

    fn submodule(&mut self, rest: &[Arg], dir: &Path) {
        let Some(pos) = rest.iter().position(|a| a.text == "foreach") else {
            return;
        };
        let cmd: Vec<Arg> = rest[pos + 1..]
            .iter()
            .skip_while(|a| a.text.starts_with('-'))
            .cloned()
            .collect();
        let script = if cmd.len() == 1 {
            cmd[0].text.clone()
        } else {
            quoted(&cmd)
        };
        self.out.push(Effect::Shell {
            script,
            dir: dir.to_path_buf(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cmd: &str) -> Vec<Effect> {
        let args: Vec<Arg> = cmd.split_whitespace().map(lit).collect();
        analyze(&args, Path::new("/r"), &|_, _, _| None)
    }

    fn blocked(cmd: &str) -> bool {
        run(cmd).iter().any(|e| matches!(e, Effect::Block(_)))
    }

    #[test]
    fn commit_flag_table() {
        for cmd in [
            "commit -n",
            "commit -nm x",
            "commit -anm x",
            "commit -m x -n",
            "commit --no-verify",
            "commit --no-veri",
            "commit --no-v",
            "commit -vn",
        ] {
            assert!(blocked(cmd), "{cmd}");
        }
        for cmd in [
            "commit -mn",
            "commit -m -n",
            "commit -F n",
            "commit -Cn",
            "commit --message -n",
            "commit --mess --no-verify",
            "commit -Sn",
            "commit -- -n",
            "commit --verify",
        ] {
            assert!(!blocked(cmd), "{cmd}");
        }
    }

    #[test]
    fn other_subcommands() {
        assert!(!blocked("log -n 5"));
        assert!(!blocked("push -n"));
        assert!(!blocked("push --dry-run"));
        assert!(blocked("push --no-verify"));
        assert!(blocked("merge --no-verify x"));
        assert!(blocked("am --no-verify"));
        assert!(blocked("rebase --no-verify main"));
        assert!(blocked("-c core.hooksPath=/dev/null commit"));
        assert!(blocked("-c CORE.HOOKSPATH=x commit"));
        assert!(blocked("--config-env=core.hooksPath=X commit"));
        let inline: Vec<Arg> = ["-c", "alias.ci=commit --no-verify", "ci"]
            .map(lit)
            .to_vec();
        assert!(matches!(
            analyze(&inline, Path::new("/r"), &|_, _, _| None)[..],
            [Effect::Block(_)]
        ));
        assert!(blocked("commit-tree abc"));
        assert!(blocked("update-ref HEAD abc"));
        assert!(blocked("replace a b"));
        assert!(!blocked("rebase main"));
        assert!(!blocked("cherry-pick abc"));
    }

    #[test]
    fn config_writes() {
        assert!(blocked("config core.hooksPath /dev/null"));
        assert!(blocked("config --local core.hookspath x"));
        assert!(blocked("config --file .git/config core.hooksPath x"));
        assert!(blocked("config set core.hooksPath x"));
        assert!(blocked("config --global --add core.hooksPath x"));
        assert!(blocked("config --edit"));
        assert!(!blocked("config --get core.hooksPath"));
        assert!(!blocked("config get core.hooksPath"));
        assert!(!blocked("config --list"));
        assert!(!blocked("config --unset core.hooksPath"));
        assert!(!blocked("config core.hooksPath"));
        assert!(!blocked("config user.name x"));
        assert!(blocked("config alias.ct commit-tree"));
        assert!(!blocked("config alias.st status"));
    }

    #[test]
    fn aliases_resolve() {
        let lookup = |_: &Path, _: &[String], name: &str| match name {
            "ci" => Some("commit --no-verify".to_string()),
            "loop" => Some("loop".to_string()),
            "st" => Some("status -sb".to_string()),
            "sh" => Some("!git commit -n".to_string()),
            _ => None,
        };
        let go = |cmd: &str| {
            let args: Vec<Arg> = cmd.split_whitespace().map(lit).collect();
            analyze(&args, Path::new("/r"), &lookup)
        };
        assert!(matches!(go("ci -m x")[..], [Effect::Block(_)]));
        assert!(matches!(go("loop")[..], [Effect::Block(_)]));
        assert!(go("st").is_empty());
        assert!(
            matches!(&go("sh x")[..], [Effect::Shell { script, .. }] if script == "git commit -n x")
        );
    }

    #[test]
    fn splits_like_git() {
        assert_eq!(
            split_cmdline(r#"commit -m "a b" --no-verify 'c d' e\ f"#).unwrap(),
            vec!["commit", "-m", "a b", "--no-verify", "c d", "e f"]
        );
        assert!(split_cmdline("commit 'x").is_none());
    }
}
