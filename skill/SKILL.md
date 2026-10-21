---
name: qgate
description: How to work in a repo gated by qgate, the local quality gate that runs as git hooks. Use when a git commit or push fails with qgate findings, when you see qgate.toml or qgate.baseline, or when asked to set qgate up in a repo.
---

<!-- qgate {{version}}. Written by `qgate install`. Do not edit: `qgate run` fails on an edited copy. Update with `qgate install`. -->

# qgate

qgate is a local, deterministic quality gate. It runs as git hooks: `git commit`
runs `qgate run --stage commit` and `git push` runs `qgate run --stage push`.
Each run executes the repo's own linters, type checkers and tests (declared in
`qgate.toml`) in parallel with qgate's built-in checks, and ends in one pass or
fail.

The rules that matter most:

- **Fix the code, never the gate.** Do not skip, disable, loosen or work around
  the hooks. `--no-verify`, `core.hooksPath`, deleting hooks, raising limits,
  growing the baseline or adding ignore comments are all either blocked or
  fail with `unapproved-change`.
- **Never run `qgate approve`.** Only the developer runs it, in their own
  terminal, after reviewing the change. The Claude Code hook blocks agents from
  running it. `qgate approve --dry-run` is allowed and shows what would need
  approval.
- **Nothing prompts.** Every command takes flags only, supports
  `--format json`, and every failure prints the command that resolves it after
  `fix:`.

Exit codes: `0` pass, `1` findings, `2` usage or config error.

## When a commit or push fails

1. Read every finding. Text output looks like
   `path:line: error[rule] message` with an optional `fix:` line. For
   structured output, rerun the same check with `--format json`:
   `qgate run --stage commit --format json` (or `--stage push`).
2. Fix the cause in the code, stage the fix, and commit again. The commit
   stage checks the staged index only, so unstaged edits are never seen.
3. Repeat until the run passes. Warnings and info findings do not fail the
   run, but act on their `fix:` when it is cheap.

What the common rules mean:

| Rule | What to do |
| --- | --- |
| `function-length` | Split the function into smaller ones. Do not move code into a fixture or excluded path. |
| `file-length` | Split the file by responsibility. |
| `duplication` | Extract the shared code into one function or module. The finding names every copy. |
| A tool's rule (eslint, clippy, mix, ruff, ...) | Fix what the tool reports. A tool that exits non-zero always fails the run, and qgate prints the tail of its output when nothing it parsed explains the failure. |
| `unapproved-change` | The change loosens the gate (see below). Undo the loosening if you can. If it is genuinely needed, stop and tell the developer exactly what needs approval and why. Do not try to approve it. |
| `tools-skipped` | Repo tools did not run because of an unapproved loosening. Resolve that first. |
| `skill-edited`, `skill-outdated`, `skill-missing` | Run `qgate install`, then commit the rewritten skill files. |

### What counts as loosening

A run fails with `unapproved-change` when the change does any of these:

- Adds or raises `qgate.baseline` entries.
- Raises a limit or the ratchet headroom, disables duplication, or adds an
  `exclude`, `fixtures` or `attribute_macros` entry in `qgate.toml`.
- Changes or removes a declared tool.
- Edits a tool's config file (such as `.eslintrc.json`, `.credo.exs`,
  `clippy.toml`), `package.json` scripts, a `pyproject.toml` `[tool]` section,
  Elixir `mix.exs` aliases or project keys, `test_helper.exs`, or a Makefile or
  justfile a tool runs.
- Marks files `linguist-generated` or `linguist-vendored`, changes a merge
  driver, or adds files under excluded paths.
- Adds an inline ignore such as `eslint-disable`, `@ts-expect-error`,
  `# noqa`, `#[allow(...)]`, `//nolint`, `credo:disable` or `@tag skip:`.

The approval covers exactly one change. Any further edit to a loosening needs
a new approval, so finish the code before asking for one.

### The baseline

`qgate.baseline` records violations that existed when qgate was adopted. It is
a ratchet: a baselined function or file may not grow (beyond any configured
`headroom_lines`), and once it shrinks it is held to the new size. New code
gets no baseline entry.

When you shrink or fix a baselined item, run `qgate baseline prune` and commit
the result. Pruning only lowers or removes entries, so it never needs approval.
Use `qgate baseline prune --dry-run` to preview.

### Push and CI

The push stage checks the tip of each pushed ref against its merge base with
the remote default branch, and runs every tool, not only those for changed
files. In CI, or in a fresh clone without hooks, run the same check by hand:

```sh
qgate run --stage push --base origin/main
```

This needs full history (`git fetch --unshallow`, or `fetch-depth: 0` in
GitHub Actions). In CI a loosening needs a signed approval record under
`.qgate/approvals/`, which only the developer can create with
`qgate approve --sign`. Agents may not write anything under `.qgate/`.

## Setting qgate up in a repo

Do these steps, then hand over to the developer:

```sh
qgate install              # git hooks, baseline merge driver, starter qgate.toml, Claude Code hook, this skill
```

Then edit `qgate.toml`: set the limits, and declare the repo's existing tools
so qgate runs them. See `qgate.toml.example` in the qgate repository for every
field. A tool entry looks like this:

```toml
[[tool]]
name = "eslint"
command = "npx eslint --format json {files}"
globs = ["**/*.{js,jsx,ts,tsx}"]
stage = "commit"        # commit tools run when their globs match a changed file; push tools always run at push
config_files = ["eslint.config.js"]   # edits to these need approval
format = "eslint-json"  # auto, sarif, eslint-json, mix, gcc or text
```

`{files}` in a commit-stage command is replaced with the changed files that
match the tool's globs. Only use it with tools that accept file paths.

```sh
qgate baseline create      # record today's violations in qgate.baseline
git add qgate.toml qgate.baseline .gitattributes .claude/skills/qgate .agents/skills/qgate
qgate approve --dry-run    # check what the developer will approve
```

Stop there. Tell the developer to review `qgate.toml` and `qgate.baseline`,
then run `qgate approve` in their own terminal and commit. Every clone needs
its own `qgate install`, because the hooks and `.claude/settings.local.json`
are local to the machine.

If `qgate install` refuses because another tool owns the hooks (husky or a
custom `core.hooksPath`), use `qgate install --chain` to keep those hooks and
run them after qgate.

## Commands

| Command | Use |
| --- | --- |
| `qgate run --stage commit` | Check the staged change, as the pre-commit hook does. |
| `qgate run --stage push [--base REF]` | Check a push, or a branch against `REF` in CI. |
| `qgate install [--chain] [--dry-run]` | Install or repair the hooks and this skill. Safe to repeat. |
| `qgate baseline create [--force] [--dry-run]` | Record current violations. The result needs approval. |
| `qgate baseline prune [--dry-run]` | Shrink the baseline after fixes. Needs no approval. |
| `qgate approve --dry-run` | Show what would need approval. Agents may only use the dry run. |
| `qgate --skill` | Print this skill for the installed version. |
| `qgate usage` | Print the whole interface as a Usage spec (usage.jdx.dev). |

Every command has `--help` with examples.
