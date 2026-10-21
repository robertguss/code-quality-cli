//! `qgate agent-hook claude`: a Claude Code PreToolUse hook that blocks
//! commands which skip or disable the git hooks, `qgate approve`, and edits to
//! the gate itself (KTD7). Protocol (KTD13): allow is exit 0 with no output,
//! block is exit 0 with a deny decision, and any error is exit 2.

pub mod git_args;
pub mod paths;
pub mod shell;

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

use git_args::Effect;
use paths::{Access, Guard};
use shell::{Node, Part, Pipeline, RedirOp, Word};

/// The hook's verdict on one tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(String),
}

impl Decision {
    /// What to print on stdout: nothing to allow, a deny decision to block.
    pub fn hook_output(&self) -> String {
        match self {
            Decision::Allow => String::new(),
            Decision::Deny(reason) => {
                let out = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason,
                    }
                });
                format!("{out}\n")
            }
        }
    }

    pub fn is_allow(&self) -> bool {
        *self == Decision::Allow
    }
}

/// Read one PreToolUse event from stdin and return what to print. Errors and
/// panics both become `Err`, which the CLI turns into exit 2.
pub fn run_claude() -> Result<String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("reading the hook event from stdin")?;
    let decision = std::panic::catch_unwind(|| handle_event(&input)).map_err(|_| {
        anyhow!("qgate agent-hook: internal error while classifying the tool call")
    })??;
    Ok(decision.hook_output())
}

/// Classify one PreToolUse event (JSON).
pub fn handle_event(input: &str) -> Result<Decision> {
    let event: Value = serde_json::from_str(input).context("hook input is not valid JSON")?;
    let tool = event
        .get("tool_name")
        .and_then(Value::as_str)
        .context("hook input has no tool_name")?;
    let process_cwd = std::env::current_dir().context("reading the current directory")?;
    let cwd = match event.get("cwd").and_then(Value::as_str) {
        Some(c) if !c.is_empty() => process_cwd.join(c),
        _ => process_cwd,
    };
    let input = event.get("tool_input");
    let field = |name: &str| -> Result<&str> {
        input
            .and_then(|i| i.get(name))
            .and_then(Value::as_str)
            .with_context(|| format!("hook input for {tool} has no tool_input.{name}"))
    };
    Ok(match tool {
        "Bash" => classify_command(field("command")?, &cwd),
        "Write" | "Edit" | "MultiEdit" => classify_file_edit(field("file_path")?, &cwd),
        "NotebookEdit" => classify_file_edit(field("notebook_path")?, &cwd),
        _ => Decision::Allow,
    })
}

/// Classify a file edit by a file-editing tool.
pub fn classify_file_edit(path: &str, cwd: &Path) -> Decision {
    let guard = Guard::for_dir(cwd);
    if guard.check(path, cwd, Access::Write) {
        Decision::Deny(reason::protected(path))
    } else {
        Decision::Allow
    }
}

/// Classify a shell command run by the Bash tool from `cwd`.
pub fn classify_command(command: &str, cwd: &Path) -> Decision {
    let guard = Guard::for_dir(cwd);
    let mut eval = Eval {
        guard: &guard,
        event_cwd: cwd.to_path_buf(),
        depth: 0,
    };
    let mut sh = Sh {
        cwd: cwd.to_path_buf(),
        vars: HashMap::new(),
    };
    match eval.script(command, &mut sh) {
        Ok(()) => Decision::Allow,
        Err(reason) => Decision::Deny(reason),
    }
}

/// The deny reasons: short, and pointing the agent at the normal path.
pub(crate) mod reason {
    pub fn bypass(what: &str) -> String {
        format!(
            "qgate: skipping or disabling git hooks is blocked (`{what}`). \
             Fix the reported findings and commit again."
        )
    }

    pub fn history(sub: &str) -> String {
        format!(
            "qgate: `git {sub}` writes commits or refs without running the hooks and is blocked. \
             Fix the reported findings and commit normally with `git commit`."
        )
    }

    pub fn approve() -> String {
        "qgate: agents may not run `qgate approve`. The developer must review and run it \
         in their own terminal; ask them to, or fix the code so no approval is needed."
            .to_string()
    }

    pub fn sign() -> String {
        "qgate: agents may not run `ssh-keygen -Y sign`, which can sign qgate approvals. \
         The developer must run `qgate approve --sign` in their own terminal."
            .to_string()
    }

    pub fn protected(what: &str) -> String {
        format!(
            "qgate: `{what}` is protected (git hooks, git config, approval records or \
             Claude Code hook settings) and agents may not change it. \
             Fix the reported findings and commit normally."
        )
    }

    pub fn marker(what: &str) -> String {
        format!(
            "qgate: this command mentions `{what}`, which touches the quality gate, and it \
             cannot be verified as safe. Fix the reported findings and commit normally."
        )
    }

    pub fn unverifiable(what: &str) -> String {
        format!(
            "qgate: `{what}` cannot be verified as safe, so it is blocked. \
             Use plain commands, fix the reported findings and commit normally."
        )
    }

    pub fn parse(err: &str) -> String {
        format!(
            "qgate: could not parse this command ({err}), so it is blocked. \
             Rewrite it as simpler commands."
        )
    }
}

/// One expanded word.
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    /// The value, with unknown expansions replaced by `DYN`.
    pub text: String,
    /// The source text.
    pub raw: String,
    /// Whether any part of the value could not be known.
    pub dynamic: bool,
}

/// Stands in for an expansion whose value is unknown.
const DYN: char = '\u{1}';

/// Whether text that cannot be expanded may carry a hook-skipping flag.
pub(crate) fn mentions_bypass(raw: &str) -> bool {
    let lc = raw.to_lowercase();
    lc.contains("no-verify") || lc.contains("hookspath")
}

/// Commands that only read, whose arguments may name protected paths freely.
const READ_ONLY: &[&str] = &[
    "cat",
    "less",
    "more",
    "head",
    "tail",
    "ls",
    "stat",
    "file",
    "wc",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "ag",
    "diff",
    "cmp",
    "readlink",
    "realpath",
    "test",
    "[",
    "[[",
    "echo",
    "printf",
    "pwd",
    "true",
    "false",
    ":",
    "which",
    "type",
    "basename",
    "dirname",
    "du",
    "tree",
    "sha256sum",
    "sha1sum",
    "md5sum",
    "cksum",
    "jq",
    "bat",
    "id",
    "uname",
    "whoami",
    "date",
];

/// Shell builtins handled here or harmless.
const BUILTINS: &[&str] = &[
    "cd", "pushd", "popd", "export", "declare", "typeset", "local", "readonly", "unset", "eval",
    "source", ".", "set", "shopt", "alias", "unalias", "exit", "return", "wait", "read", "trap",
    "shift", "break", "continue", "hash", "umask", "jobs",
];

const SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "ksh", "ash", "mksh"];

const INTERPRETERS: &[&str] = &[
    "python", "python2", "python3", "node", "perl", "ruby", "php", "lua", "Rscript", "tsx",
    "ts-node",
];

/// Commands that delete or move their path arguments.
const REMOVERS: &[&str] = &["rm", "rmdir", "unlink", "shred", "mv", "trash", "trash-put"];

/// Commands whose last argument may be a directory the others land in.
const COPIERS: &[&str] = &["cp", "mv", "ln", "install", "rsync"];

const MAX_DEPTH: usize = 16;

type Verdict = std::result::Result<(), String>;

/// Shell state that carries along a chain: directory and variables.
#[derive(Clone, Debug)]
struct Sh {
    cwd: PathBuf,
    vars: HashMap<String, String>,
}

/// Where a command's stdin comes from, when it was redirected.
#[derive(Default)]
struct Stdin {
    text: Option<String>,
    file: Option<String>,
}

struct Eval<'g> {
    guard: &'g Guard,
    event_cwd: PathBuf,
    depth: usize,
}

fn basename(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

fn lit_arg(s: &str) -> Arg {
    Arg {
        text: s.to_string(),
        raw: s.to_string(),
        dynamic: false,
    }
}

fn quoted(args: &[Arg]) -> String {
    args.iter()
        .map(|a| shell::quote(&a.text))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Environment variables that disable hooks or inject git config.
fn hook_env(name: &str, value: &Arg) -> bool {
    match name {
        "HUSKY" => value.dynamic || value.text.trim() == "0",
        "HUSKY_SKIP_HOOKS"
        // These move where git reads user and system config from.
        | "HOME"
        | "XDG_CONFIG_HOME"
        | "GIT_CONFIG_NOSYSTEM"
        | "GIT_CONFIG_COUNT"
        | "GIT_CONFIG_PARAMETERS"
        | "GIT_CONFIG_GLOBAL"
        | "GIT_CONFIG_SYSTEM"
        | "GIT_CONFIG" => true,
        n => n.starts_with("GIT_CONFIG_KEY_") || n.starts_with("GIT_CONFIG_VALUE_"),
    }
}

fn check_env(name: &str, value: &Arg) -> Verdict {
    if hook_env(name, value) {
        return Err(reason::bypass(&format!("{name}={}", value.raw)));
    }
    Ok(())
}

impl Eval<'_> {
    fn script(&mut self, src: &str, sh: &mut Sh) -> Verdict {
        if self.depth >= MAX_DEPTH {
            return Err(reason::unverifiable("deeply nested commands"));
        }
        let list = shell::parse(src).map_err(|e| reason::parse(&e))?;
        self.depth += 1;
        let result = self.list(&list, sh);
        self.depth -= 1;
        result
    }

    fn list(&mut self, list: &[Pipeline], sh: &mut Sh) -> Verdict {
        for p in list {
            if p.stages.len() > 1 || p.background {
                self.pipeline_failsafe(p)?;
                for stage in &p.stages {
                    let mut scoped = sh.clone();
                    self.node(stage, &mut scoped)?;
                }
            } else if let Some(stage) = p.stages.first() {
                self.node(stage, sh)?;
            }
        }
        Ok(())
    }

    fn node(&mut self, node: &Node, sh: &mut Sh) -> Verdict {
        match node {
            Node::Simple(cmd) => self.simple(cmd, sh),
            Node::Group {
                subshell,
                body,
                redirects,
            } => {
                for r in redirects {
                    self.redirect(r, sh)?;
                }
                if *subshell {
                    self.list(body, &mut sh.clone())
                } else {
                    self.list(body, sh)
                }
            }
        }
    }

    /// When a pipeline feeds text into something that runs it (`| sh`,
    /// `| xargs rm`), scan every word in the pipeline instead.
    fn pipeline_failsafe(&mut self, p: &Pipeline) -> Verdict {
        fn feeds(node: &Node) -> bool {
            let Node::Simple(cmd) = node else {
                return false;
            };
            let words: Vec<&str> = cmd.words.iter().map(|w| w.raw.as_str()).collect();
            let mut i = 0;
            while let Some(w) = words.get(i) {
                let name = basename(w);
                if matches!(
                    name,
                    "env" | "sudo" | "command" | "nohup" | "time" | "nice" | "exec"
                ) || w.starts_with('-')
                    || w.contains('=')
                {
                    i += 1;
                    continue;
                }
                if name == "xargs" {
                    return true;
                }
                let rest = &words[i + 1..];
                let no_script = rest
                    .iter()
                    .all(|a| a.starts_with('-') || *a == "/dev/stdin");
                return no_script
                    && (SHELLS.contains(&name)
                        || INTERPRETERS.contains(&name)
                        || matches!(name, "source" | "." | "eval"));
            }
            false
        }
        fn words(node: &Node, out: &mut String) {
            match node {
                Node::Simple(cmd) => {
                    for w in &cmd.words {
                        out.push_str(&w.raw);
                        out.push(' ');
                    }
                }
                Node::Group { body, .. } => {
                    for p in body {
                        p.stages.iter().for_each(|s| words(s, out));
                    }
                }
            }
        }
        if !p.stages.iter().any(feeds) {
            return Ok(());
        }
        let mut text = String::new();
        p.stages.iter().for_each(|s| words(s, &mut text));
        self.scan(&text)
    }

    fn var(&self, name: &str, sh: &Sh) -> Option<String> {
        if let Some(v) = sh.vars.get(name) {
            return Some(v.clone());
        }
        match name {
            "HOME" => self.guard.home.as_ref().map(|h| h.display().to_string()),
            "PWD" => Some(sh.cwd.display().to_string()),
            _ => None,
        }
    }

    /// Expand a word, classifying any command substitutions inside it.
    fn expand(&mut self, w: &Word, sh: &Sh) -> std::result::Result<Arg, String> {
        let mut text = String::new();
        let mut dynamic = false;
        for part in &w.parts {
            match part {
                Part::Quoted(s) => text.push_str(s),
                Part::Lit(s) => {
                    // Brace expansion (`--no-{a,b}`) turns one word into several.
                    if s.contains('{') && s.contains('}') {
                        dynamic = true;
                    }
                    text.push_str(s)
                }
                Part::Var(name, quoted) => match self.var(name, sh) {
                    Some(v) => {
                        // An unquoted value with spaces splits into several words,
                        // which this classifier does not model.
                        if !quoted && v.chars().any(|c| c.is_whitespace() || "*?[{".contains(c)) {
                            dynamic = true;
                        }
                        text.push_str(&v)
                    }
                    None => {
                        dynamic = true;
                        text.push(DYN);
                    }
                },
                Part::Tilde => match &self.guard.home {
                    Some(h) => text.push_str(&h.display().to_string()),
                    None => text.push('~'),
                },
                Part::Subst { body, .. } => {
                    self.list(body, &mut sh.clone())?;
                    dynamic = true;
                    text.push(DYN);
                }
                Part::Dyn(_) => {
                    dynamic = true;
                    text.push(DYN);
                }
            }
        }
        Ok(Arg {
            text,
            raw: w.raw.clone(),
            dynamic,
        })
    }

    fn redirect(&mut self, r: &shell::Redirect, sh: &Sh) -> std::result::Result<Arg, String> {
        let target = self.expand(&r.target, sh)?;
        if r.op == RedirOp::Write {
            self.write_target(&target, &sh.cwd)?;
        }
        Ok(target)
    }

    fn write_target(&self, t: &Arg, cwd: &Path) -> Verdict {
        if self.guard.check(&t.text, cwd, Access::Write) {
            return Err(reason::protected(&t.raw));
        }
        if t.dynamic {
            self.scan(&t.raw)?;
        }
        Ok(())
    }

    fn remove_target(&self, t: &Arg, cwd: &Path) -> Verdict {
        if self.guard.check(&t.text, cwd, Access::Remove) {
            return Err(reason::protected(&t.raw));
        }
        Ok(())
    }

    /// Fail-safe for anything that may run code: block text that mentions a
    /// protected path, a hook-skipping flag or `qgate approve`.
    fn scan(&self, text: &str) -> Verdict {
        if let Some(m) = self.guard.marker_in(text) {
            return Err(reason::marker(m));
        }
        let lc = text.to_lowercase();
        if lc.contains("qgate")
            && lc.match_indices("approve").any(|(i, m)| {
                !lc[i + m.len()..]
                    .trim_start_matches(|c: char| {
                        c.is_whitespace() || c == '\'' || c == '"' || c == ','
                    })
                    .starts_with("--dry-run")
            })
        {
            return Err(reason::approve());
        }
        Ok(())
    }

    fn simple(&mut self, cmd: &shell::SimpleCmd, sh: &mut Sh) -> Verdict {
        let mut assigns = Vec::new();
        for (name, w) in &cmd.assigns {
            assigns.push((name.clone(), self.expand(w, sh)?));
        }
        let mut argv = Vec::new();
        for w in &cmd.words {
            argv.push(self.expand(w, sh)?);
        }
        let mut stdin = Stdin::default();
        for r in &cmd.redirects {
            let target = self.redirect(r, sh)?;
            match r.op {
                RedirOp::Here => stdin.text = Some(target.text),
                RedirOp::Read => stdin.file = Some(target.text),
                _ => {}
            }
        }
        if argv.is_empty() {
            for (name, value) in assigns {
                check_env(&name, &value)?;
                sh.vars.insert(name, value.text);
            }
            return Ok(());
        }
        self.command(argv, assigns, stdin, sh)
    }

    fn command(
        &mut self,
        argv: Vec<Arg>,
        mut env: Vec<(String, Arg)>,
        stdin: Stdin,
        sh: &mut Sh,
    ) -> Verdict {
        let mut cwd = sh.cwd.clone();
        let mut i = 0;
        // Peel off wrappers that run their arguments as a command.
        while let Some(first) = argv.get(i) {
            let opt = |k: usize| argv.get(k).is_some_and(|a| a.text.starts_with('-'));
            match basename(&first.text) {
                "env" => {
                    i += 1;
                    while let Some(a) = argv.get(i) {
                        let t = a.text.as_str();
                        if t == "--" {
                            i += 1;
                            break;
                        }
                        if matches!(t, "-S" | "--split-string") || t.starts_with("--split-string=")
                        {
                            let (s, next) = match t.split_once('=') {
                                Some((_, v)) => (v.to_string(), i + 1),
                                None => (
                                    argv.get(i + 1).map(|a| a.text.clone()).unwrap_or_default(),
                                    i + 2,
                                ),
                            };
                            for (n, v) in &env {
                                check_env(n, v)?;
                            }
                            let rest = argv.get(next..).unwrap_or_default();
                            let script = format!("{s} {}", quoted(rest));
                            return self.script(
                                &script,
                                &mut Sh {
                                    cwd,
                                    vars: sh.vars.clone(),
                                },
                            );
                        }
                        if matches!(t, "-C" | "--chdir") {
                            if let Some(d) = argv.get(i + 1) {
                                cwd = paths::normalize(&cwd.join(&d.text));
                            }
                            i += 2;
                        } else if matches!(t, "-u" | "--unset") {
                            i += 2;
                        } else if t.starts_with('-') && t.len() > 1 {
                            i += 1;
                        } else if let Some((n, v)) = t.split_once('=')
                            && shell::is_name(n)
                        {
                            env.push((
                                n.to_string(),
                                Arg {
                                    text: v.to_string(),
                                    raw: a.raw.clone(),
                                    dynamic: a.dynamic,
                                },
                            ));
                            i += 1;
                        } else {
                            break;
                        }
                    }
                }
                "sudo" | "doas" => {
                    i += 1;
                    while opt(i) {
                        let t = argv[i].text.as_str();
                        i += 1;
                        if t == "--" {
                            break;
                        }
                        if matches!(
                            t,
                            "-u" | "-g"
                                | "-C"
                                | "-D"
                                | "-h"
                                | "-p"
                                | "-r"
                                | "-t"
                                | "-U"
                                | "-T"
                                | "-R"
                        ) {
                            i += 1;
                        }
                    }
                }
                "command" | "builtin" => {
                    i += 1;
                    while opt(i) {
                        if matches!(argv[i].text.as_str(), "-v" | "-V") {
                            return Ok(());
                        }
                        i += 1;
                    }
                }
                "exec" => {
                    i += 1;
                    while opt(i) {
                        i += if argv[i].text == "-a" { 2 } else { 1 };
                    }
                }
                "nohup" | "time" | "chronic" | "unbuffer" | "caffeinate" => {
                    i += 1;
                    while opt(i) {
                        i += 1;
                    }
                }
                "nice" | "ionice" => {
                    i += 1;
                    while opt(i) {
                        i += if matches!(argv[i].text.as_str(), "-n" | "-c" | "-p") {
                            2
                        } else {
                            1
                        };
                    }
                }
                "timeout" => {
                    i += 1;
                    while opt(i) {
                        i += if matches!(argv[i].text.as_str(), "-s" | "-k") {
                            2
                        } else {
                            1
                        };
                    }
                    i += 1;
                }
                "stdbuf" => {
                    i += 1;
                    while opt(i) {
                        i += if argv[i].text.len() == 2 { 2 } else { 1 };
                    }
                }
                "xargs" => {
                    i += 1;
                    while opt(i) {
                        let t = argv[i].text.as_str();
                        i += 1;
                        if t == "--" {
                            break;
                        }
                        if matches!(
                            t,
                            "-a" | "-d"
                                | "-E"
                                | "-I"
                                | "-L"
                                | "-n"
                                | "-P"
                                | "-s"
                                | "--arg-file"
                                | "--delimiter"
                                | "--max-args"
                                | "--max-procs"
                                | "--max-lines"
                                | "--max-chars"
                                | "--replace"
                        ) {
                            i += 1;
                        }
                    }
                }
                _ => break,
            }
        }
        let Some(first) = argv.get(i) else {
            for (n, v) in &env {
                check_env(n, v)?;
            }
            return Ok(());
        };
        let args = &argv[i..];
        let name = basename(&first.text).to_string();
        let name = name.as_str();
        if !READ_ONLY.contains(&name) {
            for (n, v) in &env {
                check_env(n, v)?;
            }
        }
        if first.dynamic {
            // A command name only known at run time could be git or qgate.
            return Err(reason::unverifiable(&format!(
                "command name `{}`",
                first.raw
            )));
        }
        if paths::is_self(&first.text, &cwd) {
            return qgate_args(&args[1..]);
        }
        let mut scoped = Sh {
            cwd: cwd.clone(),
            vars: sh.vars.clone(),
        };
        let shell_state: &mut Sh = if cwd == sh.cwd { sh } else { &mut scoped };
        match name {
            "cd" | "pushd" => return self.cd(args, shell_state),
            "export" | "declare" | "typeset" | "local" | "readonly" => {
                for a in &args[1..] {
                    if a.text.starts_with('-') {
                        continue;
                    }
                    match a.text.split_once('=') {
                        Some((n, v)) => {
                            let value = Arg {
                                text: v.to_string(),
                                raw: a.raw.clone(),
                                dynamic: a.dynamic,
                            };
                            check_env(n, &value)?;
                            shell_state.vars.insert(n.to_string(), v.to_string());
                        }
                        None => {
                            if let Some(v) = shell_state.vars.get(&a.text) {
                                check_env(&a.text, &lit_arg(v))?;
                            }
                        }
                    }
                }
                return Ok(());
            }
            "unset" => {
                for a in &args[1..] {
                    shell_state.vars.remove(&a.text);
                }
                return Ok(());
            }
            "alias" => {
                for a in &args[1..] {
                    if let Some((_, v)) = a.text.split_once('=') {
                        self.script(v, &mut shell_state.clone())?;
                    }
                }
                return Ok(());
            }
            "eval" => {
                let text: Vec<&str> = args[1..].iter().map(|a| a.text.as_str()).collect();
                return self.script(&text.join(" "), shell_state);
            }
            "source" | "." => {
                if let Some(f) = args.get(1) {
                    return self.script_file(&f.text, true, shell_state);
                }
                return Ok(());
            }
            "git" => return self.git(&args[1..], &cwd, shell_state),
            "qgate" => return qgate_args(&args[1..]),
            "ssh-keygen" => return ssh_keygen_args(&args[1..]),
            "cargo" => return cargo_args(&args[1..], &cwd),
            _ => {}
        }
        if SHELLS.contains(&name) {
            return self.shell(args, stdin, shell_state);
        }
        if BUILTINS.contains(&name) || READ_ONLY.contains(&name) {
            return Ok(());
        }
        if REMOVERS.contains(&name) {
            for a in positional(&args[1..]) {
                self.remove_target(a, &cwd)?;
            }
        }
        if matches!(name, "chmod" | "chown" | "chgrp")
            && args[1..].iter().any(|a| {
                a.text == "--recursive"
                    || (a.text.starts_with('-')
                        && !a.text.starts_with("--")
                        && a.text.contains('R'))
            })
        {
            for a in positional(&args[1..]) {
                self.remove_target(a, &cwd)?;
            }
        }
        if COPIERS.contains(&name) {
            for a in positional(&args[1..]) {
                if a.raw.to_lowercase().contains("qgate") || paths::is_self(&a.text, &cwd) {
                    return Err(reason::unverifiable(&format!(
                        "copying the qgate binary (`{}`)",
                        a.raw
                    )));
                }
            }
            self.copy_dest(name, &args[1..], &cwd)?;
        }
        if name == "find" {
            self.find(&args[1..], &cwd, shell_state)?;
        }
        if INTERPRETERS.contains(&name) || name.starts_with("python") {
            if let Some(script) = interpreter_script(&args[1..]) {
                self.script_file(&script.text, false, shell_state)?;
            }
        } else if first.text.contains('/') && !first.dynamic {
            self.script_file(&first.text, true, shell_state)?;
        }
        // An unknown program may run its arguments as a command (setsid, flock,
        // strace, watch, script -c ...): check any embedded git, qgate or shell
        // invocation, and any argument that reads as a command line.
        for (k, a) in args.iter().enumerate().skip(1) {
            let inner = basename(&a.text);
            if matches!(inner, "git" | "qgate" | "cargo") || SHELLS.contains(&inner) {
                self.command(
                    args[k..].to_vec(),
                    env.clone(),
                    Stdin::default(),
                    &mut shell_state.clone(),
                )?;
                break;
            }
            if a.text.contains(char::is_whitespace)
                && !a.dynamic
                && let Ok(list) = shell::parse(&a.text)
            {
                self.list(&list, &mut shell_state.clone())?;
            }
        }
        // The generic fail-safe: any argument naming a protected path or
        // carrying a marker blocks a command not known to be read-only.
        for a in &args[1..] {
            self.scan(&a.raw)?;
            self.scan(&a.text)?;
            if !a.text.starts_with('-') {
                self.write_target(a, &cwd)?;
            }
            if let Some((_, v)) = a.text.split_once('=') {
                self.write_target(&lit_arg(v), &cwd)?;
            }
        }
        Ok(())
    }

    fn cd(&mut self, args: &[Arg], sh: &mut Sh) -> Verdict {
        let target = args[1..]
            .iter()
            .find(|a| !a.text.starts_with('-') || a.text == "-");
        match target {
            None => {
                if let Some(h) = &self.guard.home {
                    sh.cwd = h.clone();
                }
            }
            Some(t) if t.text == "-" => {}
            Some(t) if t.dynamic => {
                let lc = t.raw.to_lowercase();
                if lc.contains("show-toplevel")
                    && let Some(root) = &self.guard.repo_root
                {
                    sh.cwd = root.clone();
                } else if ["git-dir", "git-common-dir", "git-path", ".git", ".claude"]
                    .iter()
                    .any(|m| lc.contains(m))
                {
                    return Err(reason::unverifiable(&format!("cd {}", t.raw)));
                }
            }
            Some(t) => sh.cwd = paths::normalize(&sh.cwd.join(&t.text)),
        }
        Ok(())
    }

    fn shell(&mut self, args: &[Arg], stdin: Stdin, sh: &mut Sh) -> Verdict {
        let mut j = 1;
        let mut command_string = false;
        while let Some(a) = args.get(j) {
            let t = a.text.as_str();
            if t == "--" {
                j += 1;
                break;
            }
            if t.starts_with("--") {
                j += if matches!(t, "--rcfile" | "--init-file") {
                    2
                } else {
                    1
                };
                continue;
            }
            if (t.starts_with('-') || t.starts_with('+')) && t.len() > 1 {
                command_string |= t[1..].contains('c');
                j += if t[1..].contains('o') { 2 } else { 1 };
                continue;
            }
            break;
        }
        let mut inner = sh.clone();
        match args.get(j) {
            Some(script) if command_string => {
                if script.dynamic {
                    self.scan(&script.raw)?;
                }
                self.script(&script.text, &mut inner)
            }
            Some(file) => self.script_file(&file.text, true, &mut inner),
            None => match (stdin.text, stdin.file) {
                (Some(text), _) => self.script(&text, &mut inner),
                (None, Some(file)) => self.script_file(&file, true, &mut inner),
                _ => Ok(()),
            },
        }
    }

    /// Classify a script file that is about to run: parse shell scripts, and
    /// scan anything else for markers.
    fn script_file(&mut self, path: &str, shell_script: bool, sh: &mut Sh) -> Verdict {
        let p = sh.cwd.join(path);
        let Ok(meta) = std::fs::metadata(&p) else {
            return Ok(());
        };
        if !meta.is_file() || meta.len() > 1 << 20 {
            return Ok(());
        }
        let Ok(bytes) = std::fs::read(&p) else {
            return Ok(());
        };
        if bytes.contains(&0) {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&bytes);
        let first = text.lines().next().unwrap_or("");
        let is_shell = shell_script
            && !(first.starts_with("#!") && INTERPRETERS.iter().any(|i| first.contains(i)));
        if is_shell {
            match shell::parse(&text) {
                Ok(_) => {
                    let mut inner = Sh {
                        cwd: sh.cwd.clone(),
                        vars: HashMap::new(),
                    };
                    self.script(&text, &mut inner)
                }
                Err(_) => self.scan(&text),
            }
        } else {
            self.scan(&text)
        }
    }

    fn git(&mut self, args: &[Arg], cwd: &Path, sh: &Sh) -> Verdict {
        let fallback = self.event_cwd.clone();
        let lookup = move |dir: &Path, opts: &[String], name: &str| -> Option<String> {
            let dir = if dir.is_dir() {
                dir
            } else {
                fallback.as_path()
            };
            let out = Command::new("git")
                .current_dir(dir)
                .args(opts)
                .args(["config", "--get", &format!("alias.{name}")])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        for effect in git_args::analyze(args, cwd, &lookup) {
            match effect {
                Effect::Block(why) => return Err(why),
                Effect::Shell { script, dir } => {
                    let mut inner = Sh {
                        cwd: dir,
                        vars: sh.vars.clone(),
                    };
                    self.script(&script, &mut inner)?;
                }
                Effect::Paths { paths, dir, access } => {
                    for p in paths {
                        if self.guard.check(&p, &dir, access) {
                            return Err(reason::protected(&p));
                        }
                    }
                }
                Effect::Scan(texts) => {
                    for t in texts {
                        self.scan(&t)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// `cp a b dir/` lands `dir/a`: check each landing spot.
    fn copy_dest(&self, name: &str, args: &[Arg], cwd: &Path) -> Verdict {
        let mut target_dir: Option<&Arg> = None;
        let mut files: Vec<&Arg> = Vec::new();
        let mut k = 0;
        while k < args.len() {
            let t = args[k].text.as_str();
            if matches!(t, "-t" | "--target-directory") {
                target_dir = args.get(k + 1);
                k += 2;
                continue;
            }
            if name == "install"
                && matches!(t, "-m" | "-o" | "-g" | "--mode" | "--owner" | "--group")
            {
                k += 2;
                continue;
            }
            if !t.starts_with('-') {
                files.push(&args[k]);
            }
            k += 1;
        }
        let (dest, sources): (&Arg, &[&Arg]) = match target_dir {
            Some(d) => (d, &files),
            None if files.len() >= 2 => (files[files.len() - 1], &files[..files.len() - 1]),
            None if name == "ln" && files.len() == 1 => {
                return self.write_target(&lit_arg(basename(&files[0].text)), cwd);
            }
            None => return Ok(()),
        };
        self.write_target(dest, cwd)?;
        let dest_path = cwd.join(&dest.text);
        if dest_path.is_dir() || dest.text.ends_with('/') || target_dir.is_some() {
            for s in sources {
                let landing = dest_path.join(basename(s.text.trim_end_matches('/')));
                if self
                    .guard
                    .check(&landing.to_string_lossy(), cwd, Access::Write)
                {
                    return Err(reason::protected(&dest.raw));
                }
            }
        }
        Ok(())
    }

    fn find(&mut self, args: &[Arg], cwd: &Path, sh: &mut Sh) -> Verdict {
        let mut k = 0;
        while args
            .get(k)
            .is_some_and(|a| matches!(a.text.as_str(), "-H" | "-L" | "-P"))
        {
            k += 1;
        }
        let roots: Vec<&Arg> = args[k..]
            .iter()
            .take_while(|a| !a.text.starts_with('-') && a.text != "(" && a.text != "!")
            .collect();
        let expr = &args[k + roots.len()..];
        const ACTIONS: &[&str] = &[
            "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fprintf",
            "-fls",
        ];
        if !expr.iter().any(|a| ACTIONS.contains(&a.text.as_str())) {
            return Ok(());
        }
        let expr_text = expr
            .iter()
            .map(|a| a.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        let names_gate = ["hook", "settings", "approval", ".git", ".claude", "config"]
            .iter()
            .any(|m| expr_text.contains(m));
        let default_root = [lit_arg(".")];
        let roots: Vec<&Arg> = if roots.is_empty() {
            default_root.iter().collect()
        } else {
            roots
        };
        for root in roots {
            let abs = paths::normalize(&cwd.join(&root.text));
            let holds = self.guard.check(&root.text, cwd, Access::Remove);
            let inside_repo_part = self
                .guard
                .repo_root
                .as_ref()
                .is_none_or(|r| !r.starts_with(&abs));
            if self.guard.check(&root.text, cwd, Access::Write)
                || (holds && (inside_repo_part || names_gate))
            {
                return Err(reason::protected(&root.raw));
            }
        }
        // The commands `-exec` runs are classified like any other.
        let mut k = 0;
        while k < expr.len() {
            if matches!(
                expr[k].text.as_str(),
                "-exec" | "-execdir" | "-ok" | "-okdir"
            ) {
                let cmd: Vec<Arg> = expr[k + 1..]
                    .iter()
                    .take_while(|a| a.text != ";" && a.text != "+")
                    .cloned()
                    .collect();
                k += cmd.len() + 1;
                if !cmd.is_empty() {
                    self.command(cmd, Vec::new(), Stdin::default(), sh)?;
                }
            }
            k += 1;
        }
        Ok(())
    }
}

/// Non-option arguments (everything after `--`).
fn positional(args: &[Arg]) -> Vec<&Arg> {
    let mut out = Vec::new();
    let mut dashes = false;
    for a in args {
        if dashes || !a.text.starts_with('-') {
            out.push(a);
        } else if a.text == "--" {
            dashes = true;
        }
    }
    out
}

/// The script file an interpreter would run, if it was given one.
fn interpreter_script(args: &[Arg]) -> Option<&Arg> {
    for a in args {
        let t = a.text.as_str();
        if matches!(
            t,
            "-c" | "-e" | "-E" | "-m" | "-p" | "--eval" | "--print" | "-r"
        ) {
            return None;
        }
        if !t.starts_with('-') {
            return Some(a);
        }
    }
    None
}

/// `qgate [global flags] approve ...` is blocked unless it is a dry run.
fn qgate_args(args: &[Arg]) -> Verdict {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        let t = a.text.as_str();
        i += 1;
        if t == "--format" {
            i += 1;
            continue;
        }
        if t.starts_with('-') {
            continue;
        }
        if t != "approve" && !a.dynamic {
            return Ok(());
        }
        let mut dry = false;
        let mut k = i;
        while let Some(b) = args.get(k) {
            k += 1;
            match b.text.as_str() {
                "--commit" | "--format" | "--base" | "--key" => k += 1,
                "--dry-run" => dry = true,
                "--" => break,
                _ => {}
            }
        }
        return if dry && !a.dynamic {
            Ok(())
        } else {
            Err(reason::approve())
        };
    }
    Ok(())
}

/// `ssh-keygen -Y sign` can sign a committed approval record with the
/// developer's key, so agents may not run it.
fn ssh_keygen_args(args: &[Arg]) -> Verdict {
    let signs = args.iter().enumerate().any(|(i, a)| {
        a.dynamic
            || a.text == "-Ysign"
            || (a.text == "-Y"
                && args
                    .get(i + 1)
                    .is_none_or(|n| n.dynamic || n.text == "sign"))
    });
    if signs {
        return Err(reason::sign());
    }
    Ok(())
}

/// `cargo run ... -- approve` runs this crate's `qgate approve`, and
/// `cargo install` of qgate replaces the binary every hook runs.
fn cargo_args(args: &[Arg], cwd: &Path) -> Verdict {
    let sub = args
        .iter()
        .find(|a| !a.text.starts_with('+') && !a.text.starts_with('-'));
    if sub.is_some_and(|s| s.text == "install") {
        let names_qgate = args.iter().any(|a| {
            let t = a.raw.to_lowercase();
            t.contains("qgate") || t.contains("code-quality-cli")
        });
        let path_is_qgate = args.windows(2).any(|w| {
            w[0].text == "--path"
                && std::fs::read_to_string(cwd.join(&w[1].text).join("Cargo.toml"))
                    .is_ok_and(|t| t.contains("name = \"qgate\""))
        });
        if names_qgate || path_is_qgate {
            return Err(reason::unverifiable("reinstalling the qgate binary"));
        }
    }
    if sub.is_some_and(|s| s.text == "run" || s.text == "r")
        && let Some(pos) = args.iter().position(|a| a.text == "--")
    {
        return qgate_args(&args[pos + 1..]);
    }
    Ok(())
}
