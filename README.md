# qgate

A local, deterministic quality gate for code written by AI agents. It runs as
git hooks on every commit and push, so the mechanical checks that usually wait
on remote CI finish on your machine in seconds, and AI review can spend its
tokens on judgment instead.

- **One gate, one result.** Your repo's own linters, type checkers and tests run
  in parallel alongside qgate's built-in checks, and every run ends in a single
  pass or fail.
- **Built-in checks in any language.** Function length, file length and
  duplicated code across the whole repo. Function lengths use 13 bundled
  tree-sitter grammars: Rust, Python, JavaScript, TypeScript/TSX, Go, Java, C,
  C++, Ruby, Bash, C# and PHP. Every other text file still gets the file-length
  check.
- **A ratchet, not a cliff.** Existing violations go in a baseline that can only
  shrink, so a legacy repo passes on day one and new or worse code fails.
- **Agents can't skip or loosen the gate.** Raising a limit, growing the
  baseline, editing a linter config, excluding files or adding an
  `eslint-disable` all fail until a developer runs `qgate approve`. A Claude
  Code hook blocks agents from running that command, from `--no-verify`, and
  from tampering with the hooks.
- **Built for agents to call.** Nothing ever prompts. Every command has
  `--help` with examples and supports `--format json`. Every failure prints the
  command that resolves it. `qgate usage` prints the whole interface as a
  [Usage spec](https://usage.jdx.dev).

## Install

```sh
cargo install --locked --git https://github.com/robertguss/code-quality-cli --tag v0.1.0 qgate
```

## Set up a repo

Run these in your own terminal, not through an agent:

```sh
cd your-repo
qgate install            # hooks, merge driver, starter qgate.toml, Claude Code hook
$EDITOR qgate.toml       # set limits and declare your tools (see qgate.toml.example)
qgate baseline create    # record today's violations in qgate.baseline
git add qgate.toml qgate.baseline .gitattributes .claude/settings.json
qgate approve            # approve the new config and baseline
git commit -m "Add qgate"
```

From then on, `git commit` runs `qgate run --stage commit` and `git push` runs
`qgate run --stage push`.

`install` refuses when another tool already owns the hooks, such as husky or a
custom `core.hooksPath`. Use `qgate install --chain` to keep those hooks and run
them after qgate. Run `install` again after moving the binary, because the hooks
call it by absolute path.

## What runs when

| | Commit | Push |
|---|---|---|
| Content checked | The staged index, never unstaged edits | The tip of each pushed ref |
| Compared against | `HEAD` | The merge base with the remote default branch |
| Built-in checks | Changed files. Duplication is found repo-wide and reported only when a copy touches the change | Same, over the whole push |
| Repo tools | `stage = "commit"` tools whose globs match changed files | Every tool |
| Guard | The staged change | Each pushed commit against its first parent |

Repo tools run in the working tree when it matches what is staged. Otherwise
they run in a temporary checkout of the staged tree, with `node_modules`,
`.venv`, `target` and similar directories linked in. qgate never stashes, never
re-stages and never edits your files.

## The baseline

`qgate.baseline` is a sorted text file with one entry per existing violation:

- Function entries are keyed by path and qualified name.
- File entries are keyed by path.
- Duplicate-block entries are keyed by a hash of the shared code, so moving a
  copy keeps its key.

The ratchet works like this:

- A baselined violation may not grow. Once it shrinks, it is held to its new
  size immediately, even before anyone prunes.
- `qgate baseline prune` lowers or removes entries that shrank or were fixed,
  and follows renamed files and moved functions. It never adds or raises an
  entry, so its output needs no approval. Use `--dry-run` to preview.
- `qgate install` registers a merge driver that resolves baseline conflicts by
  keeping the lower value.

## Approvals

A run fails with `unapproved-change` when the change does any of these:

- Adds or raises baseline entries.
- Raises a limit, disables duplication or adds an `exclude` pattern.
- Changes or removes a declared tool.
- Edits a tool's config file. This includes any file with the same name
  elsewhere in the repo, such as a nested `.eslintrc.json`.
- Edits `package.json` scripts or a `pyproject.toml` `[tool]` section.
- Edits a Makefile or justfile when a tool runs `make` or `just`.
- Marks files `linguist-generated` or `linguist-vendored`, changes a merge
  driver, or adds files under excluded paths.
- Adds an inline ignore such as `eslint-disable`, `@ts-expect-error`, `# noqa`,
  `#[allow(...)]` or `//nolint`, or one of your `ignore_patterns`.

Review the change, then run `qgate approve` yourself. It records a hash of
exactly that loosening in `.git/qgate/approvals/`, which is never committed, so
any further edit needs a new approval. Use `qgate approve --dry-run` to see what
would be approved. Use `qgate approve --commit <sha>` for a commit that reached
the push hook without going through pre-commit, for example after a cherry-pick.

## Known limits

qgate stops agent shortcuts. It does not stop a determined human or every
possible agent:

- Anyone with a shell can delete the hooks. A Claude Code setting can also turn
  off non-managed hooks.
- The agent hook covers Claude Code only. It inspects commands the agent runs
  directly. Commands run indirectly are not inspected, such as a script file, a
  `make` or `npm run` target, or an interpreter running code built at runtime.
  Signed or hardware-key approvals, which would close this gap, are planned.
- Rebase, cherry-pick, revert, `git am` and clean merges do not run
  pre-commit. The push hook is the backstop for those.
- Clones without `qgate install` are not gated.
- husky regenerates its hook directory on `npm install`, so re-run
  `qgate install --chain` afterwards.
- The repo's own tools must already be installed. qgate runs them but does not
  install them.
