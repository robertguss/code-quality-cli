mod support;

use std::path::Path;

use qgate::agent_hook::{Decision, classify_command, classify_file_edit, handle_event};
use serde_json::{Value, json};
use support::repo::TestRepo;

fn repo() -> TestRepo {
    let t = TestRepo::new();
    t.write("src/main.rs", "fn main() {}\n");
    t.write(
        ".git/hooks/pre-commit",
        "#!/bin/sh\nexec qgate run --stage commit\n",
    );
    std::fs::create_dir_all(t.path().join("sub")).unwrap();
    t
}

#[track_caller]
fn assert_blocked(t: &TestRepo, cmd: &str) {
    let d = classify_command(cmd, t.path());
    assert!(matches!(d, Decision::Deny(_)), "expected block: {cmd}");
}

#[track_caller]
fn assert_allowed(t: &TestRepo, cmd: &str) {
    let d = classify_command(cmd, t.path());
    assert_eq!(d, Decision::Allow, "expected allow: {cmd}");
}

fn bash_event(cmd: &str, cwd: &Path) -> String {
    json!({
        "session_id": "s",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": cmd, "description": "x"},
        "cwd": cwd,
    })
    .to_string()
}

fn edit_event(tool: &str, path: &str, cwd: &Path) -> String {
    let key = if tool == "NotebookEdit" {
        "notebook_path"
    } else {
        "file_path"
    };
    json!({"tool_name": tool, "tool_input": {key: path, "content": "x"}, "cwd": cwd}).to_string()
}

/// Run the hook binary and return (exit code, stdout, stderr).
fn hook(t: &TestRepo, event: &str, home: Option<&Path>) -> (i32, String, String) {
    let mut cmd = t.qgate();
    cmd.args(["agent-hook", "claude"]).write_stdin(event);
    if let Some(h) = home {
        cmd.env("HOME", h);
    }
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// The deny reason the binary printed, or `None` when it allowed.
fn hook_reason(t: &TestRepo, event: &str, home: Option<&Path>) -> Option<String> {
    let (code, stdout, stderr) = hook(t, event, home);
    assert_eq!(code, 0, "stderr: {stderr}");
    if stdout.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(&stdout).unwrap();
    let out = &v["hookSpecificOutput"];
    assert_eq!(out["hookEventName"], "PreToolUse");
    assert_eq!(out["permissionDecision"], "deny");
    Some(
        out["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .to_string(),
    )
}

// Protocol

#[test]
fn allow_is_exit_zero_with_no_output() {
    let t = repo();
    let (code, stdout, _) = hook(&t, &bash_event("git status", t.path()), None);
    assert_eq!((code, stdout.as_str()), (0, ""));
}

#[test]
fn block_is_a_deny_decision_telling_the_agent_to_fix_the_code() {
    // AE3
    let t = repo();
    let reason = hook_reason(
        &t,
        &bash_event("git commit --no-verify -m x", t.path()),
        None,
    )
    .expect("blocked");
    assert!(reason.contains("Fix the reported findings"), "{reason}");
    assert!(reason.len() < 300, "{reason}");
}

#[test]
fn malformed_input_exits_2() {
    let t = repo();
    for bad in [
        "not json",
        "{\"tool_name\": ",
        "{}",
        r#"{"tool_name":"Bash","tool_input":{}}"#,
    ] {
        let (code, stdout, stderr) = hook(&t, bad, None);
        assert_eq!(code, 2, "{bad}");
        assert!(stdout.is_empty());
        assert!(!stderr.is_empty());
    }
}

#[test]
fn unknown_tools_and_empty_commands_are_allowed() {
    let t = repo();
    let read = json!({"tool_name": "Read", "tool_input": {"file_path": ".git/config"}}).to_string();
    assert_eq!(hook_reason(&t, &read, None), None);
    assert_eq!(hook_reason(&t, &bash_event("", t.path()), None), None);
    assert_eq!(hook_reason(&t, &bash_event("  \n", t.path()), None), None);
}

#[test]
fn library_entry_point_reads_the_event() {
    let t = repo();
    let d = handle_event(&bash_event("git push --no-verify", t.path())).unwrap();
    assert!(matches!(d, Decision::Deny(_)));
    assert!(handle_event("[").is_err());
}

#[test]
fn unparseable_commands_fail_closed() {
    let t = repo();
    assert_blocked(&t, "echo 'unterminated");
    assert_blocked(&t, "git commit -m \"x");
    assert_blocked(&t, "echo $(git status");
}

// Hook bypasses (R12)

#[test]
fn commit_no_verify_forms_are_blocked() {
    // AE3
    let t = repo();
    for cmd in [
        "git commit --no-verify -m x",
        "git commit -nm x",
        "git commit -anm x",
        "git commit -m x -n",
        "git commit --no-veri -m x",
        "git -c core.hooksPath=/dev/null commit",
        "git -c CORE.HOOKSPATH=/dev/null commit -m x",
        "git --config-env=core.hooksPath=EVIL commit -m x",
        "git -C sub commit --no-verify",
        "git --no-pager -p commit -n -m x",
        "/usr/bin/git commit --no-verify",
        "sudo git commit -n -m x",
        "command git commit -n",
        "\\git commit -n",
        "git commit $(echo --no-verify) -m x",
    ] {
        assert_blocked(&t, cmd);
    }
}

#[test]
fn harmless_short_flags_are_allowed() {
    // AE6
    let t = repo();
    for cmd in [
        "git log -n 5",
        "git push --dry-run",
        "git push -n origin main",
        "git status",
        "git commit -mn",
        "git commit -m 'skip -n and --no-verify talk'",
        "git --no-pager log --oneline -n 3",
        "git rebase main",
        "git cherry-pick abc",
        "git merge --no-ff feature",
        "git revert HEAD",
        "git am patch.mbox",
        "git config --get core.hooksPath",
        "git config get core.hooksPath",
        "git config --list",
        "git config --unset core.hooksPath",
        "git commit -m \"$(cat <<'EOF'\nfeat: x\n\nmentions .git/hooks and --no-verify\nEOF\n)\"",
    ] {
        assert_allowed(&t, cmd);
    }
}

#[test]
fn push_and_other_no_verify_are_blocked() {
    let t = repo();
    for cmd in [
        "git push --no-verify",
        "git push origin main --no-verify",
        "git merge --no-verify feature",
        "git am --no-verify x.mbox",
        "git rebase --no-verify main",
        "git pull --no-verify",
    ] {
        assert_blocked(&t, cmd);
    }
}

#[test]
fn hook_disabling_environment_is_blocked() {
    let t = repo();
    for cmd in [
        "HUSKY=0 git commit -m x",
        "HUSKY_SKIP_HOOKS=1 git commit -m x",
        "export HUSKY=0 && git commit -m x",
        "HUSKY=0; export HUSKY; git commit -m x",
        "env HUSKY=0 git commit -m x",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0=/dev/null git commit -m x",
        "export GIT_CONFIG_COUNT=1",
        "GIT_CONFIG_PARAMETERS=\"'core.hookspath'='/dev/null'\" git commit -m x",
        "GIT_CONFIG_GLOBAL=/tmp/evil git commit -m x",
    ] {
        assert_blocked(&t, cmd);
    }
    assert_allowed(&t, "HUSKY=1 git commit -m x");
    assert_allowed(&t, "GIT_PAGER=cat git log");
}

#[test]
fn chained_and_nested_commands_are_seen() {
    let t = repo();
    for cmd in [
        "cd sub && git commit --no-verify",
        "bash -c \"git commit --no-verify\"",
        "sh -ec 'git commit -n -m x'",
        "zsh -c 'git push --no-verify'",
        "eval 'git commit --no-verify'",
        "true; (git status; git commit -n)",
        "{ git status; git commit -n; }",
        "echo $(git commit --no-verify)",
        "echo `git commit -n`",
        "git status | git commit -n",
        "if true; then git commit -n; fi",
        "f() { git commit -n; }; f",
        "bash <<'EOF'\ngit commit --no-verify\nEOF",
        "echo 'git commit --no-verify' | sh",
        "env -S 'git commit --no-verify'",
        "nohup git commit -n &",
        "timeout 10 git commit -n",
    ] {
        assert_blocked(&t, cmd);
    }
}

#[test]
fn history_writing_plumbing_is_blocked() {
    let t = repo();
    assert_blocked(&t, "git commit-tree HEAD^{tree} -m x");
    assert_blocked(&t, "git update-ref refs/heads/main abc");
    assert_blocked(&t, "git replace abc def");
    assert_allowed(&t, "git rebase main");
    assert_allowed(&t, "git cherry-pick abc");
}

#[test]
fn hooks_path_config_writes_are_blocked() {
    let t = repo();
    for cmd in [
        "git config core.hooksPath /dev/null",
        "git config --local core.hooksPath /dev/null",
        "git config --global core.hooksPath /tmp/nohooks",
        "git config --worktree core.hooksPath x",
        "git config --file .git/config core.hooksPath x",
        "git config set core.hooksPath /dev/null",
        "git config --edit",
    ] {
        assert_blocked(&t, cmd);
    }
}

#[test]
fn alias_definitions_that_skip_hooks_are_blocked() {
    let t = repo();
    for cmd in [
        "git config alias.ci 'commit --no-verify'",
        "git config --global alias.ci 'commit -n'",
        "git config alias.p 'push --no-verify'",
        "git config alias.nh '-c core.hooksPath=/dev/null commit'",
        "git config alias.ct commit-tree",
        "git config alias.x '!git commit --no-verify'",
        "git -c alias.ci='commit --no-verify' ci -m x",
    ] {
        assert_blocked(&t, cmd);
    }
    assert_allowed(&t, "git config alias.st 'status -sb'");
}

#[test]
fn existing_aliases_are_resolved_before_classifying() {
    let t = repo();
    t.git(&["config", "alias.ci", "commit --no-verify"]);
    t.git(&["config", "alias.cm", "commit -m"]);
    t.git(&["config", "alias.sh", "!git commit -n"]);
    t.git(&["config", "alias.outer", "ci"]);
    t.git(&["config", "alias.st", "status -sb"]);
    for cmd in ["git ci -m x", "git outer -m x", "git sh", "git cm x -n"] {
        assert!(
            hook_reason(&t, &bash_event(cmd, t.path()), None).is_some(),
            "{cmd}"
        );
    }
    assert_eq!(hook_reason(&t, &bash_event("git st", t.path()), None), None);
    assert_eq!(
        hook_reason(&t, &bash_event("git cm 'n'", t.path()), None),
        None
    );
}

// Approval (R14, AE5)

#[test]
fn qgate_approve_is_blocked() {
    // AE5
    let t = repo();
    for cmd in [
        "qgate approve",
        "qgate --format json approve",
        "/usr/local/bin/qgate approve --commit abc",
        "./target/debug/qgate approve",
        "env FOO=1 qgate approve",
        "cd sub && qgate approve",
        "cargo run -q -- approve",
        "bash -c 'qgate approve'",
        "python3 -c \"import subprocess; subprocess.run(['qgate', 'approve'])\"",
    ] {
        assert_blocked(&t, cmd);
    }
    let reason = hook_reason(&t, &bash_event("qgate approve", t.path()), None).unwrap();
    assert!(reason.contains("own terminal"), "{reason}");
    assert_allowed(&t, "qgate approve --dry-run");
    assert_allowed(&t, "qgate run --stage commit");
    assert_allowed(&t, "qgate baseline create && qgate approve --dry-run");
}

#[test]
fn writes_into_the_approval_store_are_blocked() {
    // AE5
    let t = repo();
    for tool in ["Write", "Edit", "MultiEdit"] {
        let reason = hook_reason(
            &t,
            &edit_event(tool, ".git/qgate/approvals/abc", t.path()),
            None,
        );
        assert!(reason.is_some(), "{tool}");
    }
    let abs = t.path().join(".git/qgate/approvals/abc");
    assert!(classify_file_edit(abs.to_str().unwrap(), Path::new("/")) != Decision::Allow);
}

// Protected paths (R19)

#[test]
fn edits_to_claude_settings_are_blocked() {
    let t = repo();
    let home = tempfile::tempdir().unwrap();
    for path in [
        ".claude/settings.json",
        ".claude/settings.local.json",
        "sub/.claude/settings.json",
    ] {
        assert!(
            hook_reason(&t, &edit_event("Write", path, t.path()), Some(home.path())).is_some(),
            "{path}"
        );
    }
    let user = home.path().join(".claude/settings.json");
    assert!(
        hook_reason(
            &t,
            &edit_event("Edit", user.to_str().unwrap(), t.path()),
            Some(home.path())
        )
        .is_some()
    );
    let tilde = bash_event("echo '{}' > ~/.claude/settings.json", t.path());
    assert!(hook_reason(&t, &tilde, Some(home.path())).is_some());
    let var = bash_event("cp x \"$HOME/.claude/settings.local.json\"", t.path());
    assert!(hook_reason(&t, &var, Some(home.path())).is_some());
}

#[test]
fn edits_to_git_config_and_hooks_are_blocked() {
    let t = repo();
    for path in [
        ".git/config",
        ".git/hooks/pre-commit",
        ".git/info/exclude",
        "sub/../.git/config",
    ] {
        assert!(
            matches!(classify_file_edit(path, t.path()), Decision::Deny(_)),
            "{path}"
        );
    }
    let nb = edit_event("NotebookEdit", ".git/hooks/x.ipynb", t.path());
    assert!(hook_reason(&t, &nb, None).is_some());
}

#[test]
fn ordinary_edits_and_reads_are_allowed() {
    let t = repo();
    assert_eq!(classify_file_edit("src/main.rs", t.path()), Decision::Allow);
    assert_eq!(
        classify_file_edit(".claude/agents/reviewer.md", t.path()),
        Decision::Allow
    );
    assert_eq!(
        classify_file_edit(".git/COMMIT_EDITMSG", t.path()),
        Decision::Allow
    );
    for cmd in [
        "cat .git/config",
        "ls -la .git/hooks",
        "head -5 .git/hooks/pre-commit",
        "grep hooksPath .git/config",
        "cargo test && cargo clippy --all-targets -- -D warnings",
        "rm -rf target",
        "find . -name '*.orig' -delete",
        "echo hi > notes.txt",
        "cp src/main.rs /tmp/main.rs.bak",
        "mkdir -p .claude && ls .claude",
        "git checkout -- src/main.rs",
        "git checkout .",
        "git add .claude/settings.json && git commit -m 'add hook settings'",
        "for f in src/*.rs; do wc -l $f; done",
        "cd \"$(git rev-parse --show-toplevel)\" && cargo build",
    ] {
        assert_allowed(&t, cmd);
    }
}

#[test]
fn shell_writes_to_protected_paths_are_blocked() {
    // AE11
    let t = repo();
    for cmd in [
        "rm .git/hooks/pre-commit",
        "rm -f -- .git/hooks/pre-commit",
        "echo x > .git/qgate/approvals/a",
        "echo '{}' > .claude/settings.local.json",
        "cat new.json | tee .claude/settings.local.json",
        "printf '[core]\\n hooksPath = /dev/null\\n' >> .git/config",
        "echo x 2> .git/hooks/pre-push",
        "echo x &> .git/hooks/pre-push",
        "mv .git/hooks/pre-commit /tmp/",
        "mv .git/hooks /tmp/hooks",
        "rm -rf .git",
        "rm -rf .claude",
        "rm .git/hooks/*",
        "rm .git/h*/pre-commit",
        "cp /tmp/evil .git/hooks/pre-commit",
        "cp /tmp/pre-commit .git/hooks/",
        "ln -sf /dev/null .git/hooks/pre-commit",
        "install -m 755 /tmp/x .git/hooks/pre-commit",
        "truncate -s 0 .git/hooks/pre-commit",
        "chmod -x .git/hooks/pre-commit",
        "touch .git/qgate/approvals/x",
        "dd if=/dev/null of=.git/hooks/pre-commit",
        "sed -i 's/qgate/true/' .git/hooks/pre-commit",
        "perl -pi -e 's/qgate/true/' .git/hooks/pre-commit",
        "find .git/hooks -type f -delete",
        "find .git -name pre-commit -exec rm {} \\;",
        "unlink .git/hooks/pre-commit",
        "cd .git && rm hooks/pre-commit",
        "cd .git/hooks; rm pre-commit",
        "D=.git/hooks; rm $D/pre-commit",
        "rm \"$PWD/.git/hooks/pre-commit\"",
        "python3 -c \"open('.git/hooks/pre-commit','w').write('')\"",
        "node -e \"require('fs').unlinkSync('.git/hooks/pre-commit')\"",
        "echo .git/hooks/pre-commit | xargs rm",
        "git rm .claude/settings.json",
        "git clean -fdx",
        "vim .git/config",
    ] {
        assert_blocked(&t, cmd);
    }
}

#[test]
fn git_checkout_of_protected_paths_is_blocked() {
    let t = repo();
    assert_blocked(&t, "git checkout HEAD~1 -- .claude/settings.json");
    assert_blocked(&t, "git restore --source HEAD~1 .claude/settings.json");
    assert_blocked(&t, "git -C sub checkout main -- ../.claude/settings.json");
    assert_allowed(&t, "git checkout HEAD~1 -- src/main.rs");
}

#[test]
fn symlinks_do_not_dodge_the_protected_list() {
    let t = repo();
    std::os::unix::fs::symlink(t.path().join(".git/hooks"), t.path().join("h")).unwrap();
    std::os::unix::fs::symlink(t.path().join(".git/config"), t.path().join("cfg")).unwrap();
    assert!(matches!(
        classify_file_edit("h/pre-commit", t.path()),
        Decision::Deny(_)
    ));
    assert!(matches!(
        classify_file_edit("cfg", t.path()),
        Decision::Deny(_)
    ));
    assert_blocked(&t, "echo x > h/pre-commit");
    assert_blocked(&t, "rm h/pre-commit");
}

#[test]
fn script_files_are_classified_before_they_run() {
    let t = repo();
    t.write(
        "bypass.sh",
        "#!/bin/sh\nset -e\ngit add -A\ngit commit --no-verify -m wip\n",
    );
    t.write("ok.sh", "#!/bin/sh\ncargo test\n");
    t.write("evil.py", "import os\nos.remove('.git/hooks/pre-commit')\n");
    assert_blocked(&t, "bash bypass.sh");
    assert_blocked(&t, "sh ./bypass.sh");
    assert_blocked(&t, "source bypass.sh");
    assert_blocked(&t, "./bypass.sh");
    assert_blocked(&t, "python3 evil.py");
    assert_allowed(&t, "bash ok.sh");
}

#[test]
fn worktree_hooks_live_in_the_common_git_dir() {
    let t = repo();
    t.commit_all("init");
    let wt = tempfile::tempdir().unwrap();
    let wt_path = wt.path().join("wt");
    t.git(&["worktree", "add", "-q", wt_path.to_str().unwrap()]);
    let hooks = t.path().join(".git/hooks/pre-commit");
    let cmd = format!("rm {}", hooks.display());
    assert!(matches!(
        classify_command(&cmd, &wt_path),
        Decision::Deny(_)
    ));
    assert_allowed(&t, "git worktree list");
    assert!(classify_command("git commit -n", &wt_path) != Decision::Allow);
}

// Review regressions: expansions, wrappers, the binary and user git config

#[test]
fn expansions_that_hide_flags_are_blocked() {
    let t = repo();
    for cmd in [
        "a=\"commit --no-verify\"; git $a -m x",
        "a=\"push --no-verify\"; git $a",
        "git commit --no-{verify,x} -m x",
        "git commit $'\\x2d-no-verify' -m x",
        "git commit $(printf -- '--no-%s' verify) -m x",
        "\"$(which git)\" commit -n -m x",
    ] {
        assert_blocked(&t, cmd);
    }
    assert_allowed(&t, "git commit -m \"$MSG\"");
    assert_allowed(&t, "git log -n 5");
    assert_allowed(&t, "git push --dry-run");
}

#[test]
fn process_wrappers_are_seen_through() {
    let t = repo();
    for cmd in [
        "setsid git commit -n -m x",
        "watch 'git commit -n -m x'",
        "script -qc 'git commit -n -m x' /dev/null",
        "flock /tmp/l git commit -n -m x",
        "strace -f git commit -n -m x",
        "taskset -c 0 git commit -n -m x",
        "chrt -o 0 git commit -n -m x",
    ] {
        assert_blocked(&t, cmd);
    }
}

#[test]
fn approve_cannot_hide_behind_comments() {
    let t = repo();
    assert_blocked(
        &t,
        "python3 -c \"import os; os.system('qgate approve') # --dry-run\"",
    );
}

#[test]
fn the_qgate_binary_is_protected() {
    let t = repo();
    let exe = env!("CARGO_BIN_EXE_qgate");
    let canon = Path::new(exe).canonicalize().unwrap();
    let canon = canon.to_str().unwrap();
    for cmd in [
        format!("cp /bin/true {canon}"),
        format!("ln -sf {canon} ./q && ./q approve"),
        format!("cp {canon} ./q && ./q approve"),
        format!("{canon} approve"),
        format!("cargo install --path {}", env!("CARGO_MANIFEST_DIR")),
    ] {
        assert!(
            hook_reason(&t, &bash_event(&cmd, t.path()), None).is_some(),
            "{cmd}"
        );
    }
    assert!(hook_reason(&t, &edit_event("Write", canon, t.path()), None).is_some());
}

#[test]
fn user_git_config_is_protected() {
    let t = repo();
    let home = tempfile::tempdir().unwrap();
    for cmd in [
        "echo '[core] hooksPath=/tmp' >> ~/.gitconfig",
        "printf '[core]\\n' >> ~/.config/git/config",
        "HOME=/tmp/h git commit -m x",
        "git config core.attributesFile /tmp/a",
        "git -c core.attributesFile=/tmp/a commit -m x",
    ] {
        assert!(
            hook_reason(&t, &bash_event(cmd, t.path()), Some(home.path())).is_some(),
            "{cmd}"
        );
    }
    assert_eq!(
        hook_reason(
            &t,
            &bash_event("cat ~/.gitconfig", t.path()),
            Some(home.path())
        ),
        None
    );
}

#[test]
fn signing_and_approval_records_are_blocked() {
    let t = repo();
    assert_blocked(
        &t,
        "ssh-keygen -Y sign -n qgate -f ~/.ssh/id_ed25519 < text",
    );
    assert_blocked(&t, "ssh-keygen -Ysign -n qgate -f key");
    assert_blocked(&t, "qgate approve --base origin/main --sign");
    assert_blocked(&t, "echo forged > .qgate/approvals/abc.sig");
    assert_blocked(&t, "rm -rf .qgate");
    assert_allowed(&t, "ssh-keygen -t ed25519 -N '' -f /tmp/k");
    assert_allowed(&t, "qgate approve --base origin/main --dry-run");
    assert!(!classify_file_edit(".qgate/allowed_signers", t.path()).is_allow());
}
