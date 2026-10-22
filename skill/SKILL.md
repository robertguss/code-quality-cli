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

- **Fix the code, not the gate.** Never skip, disable or work around the
  hooks: `--no-verify`, `core.hooksPath` and deleting hooks are blocked.
  Raising limits, growing the baseline or adding ignore comments is allowed,
  but each one shows as a `loosening` warning. Loosening a correctness rule
  (see "Suppressions" below) fails instead. Prefer fixing the code, and when
  you do loosen the gate, tell the developer what you loosened and why.
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
| `escape-hatch/*` | Remove the escape hatch (`as` casts, `any`, `.unwrap()`, `panic!`, untyped public functions, ...). The finding's message names the replacement. |
| `swallowed-error/*` | Handle, propagate or log the error. Never silence these: their suppressions always fail. |
| `debug-output/*` | Remove the debug print, or return or log the value. Printing is allowed in a binary's main file, scripts, and paths the repo lists in `[rules.debug-output] allow`. |
| `suppression` | An inline suppression needs `reason:` and `until:` on its line (see "Suppressions"), or has expired. |
| `loosening` | The change loosens the gate (see below). A warning, except for the correctness rules, where it is an error. Undo it if fixing the code is reasonable; otherwise keep it and mention it to the developer. |
| `skill-edited`, `skill-outdated`, `skill-missing` | Run `qgate install`, then commit the rewritten skill files. |

### What counts as loosening

A run warns with `loosening` when the change does any of these:

- Adds or raises `qgate.baseline` entries (an error for `always_block` rules).
- Lowers or turns off a rule in `[rules]`, or adds a `[rules.debug-output]
  allow` glob.
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

### Rules and suppressions

`qgate rules` lists every rule with its id, kind, level in this repo, source
and rationale; `qgate rules <id>` explains one. Rule ids look like
`escape-hatch/as-cast`: the part before `/` is the family. Exact rules fail the
run; heuristic rules warn, since they can misfire. `qgate.toml` sets levels in
`[rules]`, and lowering one is a loosening.

Occurrence rules (`escape-hatch`, `swallowed-error`, `debug-output`) are
counted per file. A file's count may not rise above its baseline entry or its
count before the change, so new code gets none and existing debt can only
shrink. Test files and Rust `#[cfg(test)]` code are not counted for rules that
only apply to shipped code.

A new inline suppression comment, such as `eslint-disable-next-line`, `# noqa`,
`//nolint` or qgate's own marker, must say why and until when on the same
line, at most 90 days ahead (`[suppressions] max_days`):

```ts
// eslint-disable-next-line no-console -- reason: CLI output, until: 2026-12-31
// qgate: allow escape-hatch/as-cast -- reason: library type is wrong, until: 2026-12-31
```

`qgate: allow <rule or family>` on a line, or the line above it, takes that
occurrence out of qgate's count. Expired suppressions fail when their file is
changed. Suppressions of anything on `[suppressions] always_block` (by default
the exact `swallowed-error` rules, `@ts-ignore`, `@ts-nocheck` and a bare
`eslint-disable`) always fail, and so do config or baseline edits that lower,
remove or grow those rules. Fix the code instead.

### The baseline

`qgate.baseline` records violations that existed when qgate was adopted. It is
a ratchet: a baselined function or file may not grow (beyond any configured
`headroom_lines`), and once it shrinks it is held to the new size. New code
gets no baseline entry.

When you shrink or fix a baselined item, run `qgate baseline prune` and commit
the result. Pruning only lowers or removes entries, so it is never a
loosening. Use `qgate baseline prune --dry-run` to preview.

### Push and CI

The push stage checks the tip of each pushed ref against its merge base with
the remote default branch, and runs every tool, not only those for changed
files. In CI, or in a fresh clone without hooks, run the same check by hand:

```sh
qgate run --stage push --base origin/main
```

This needs full history (`git fetch --unshallow`, or `fetch-depth: 0` in
GitHub Actions).

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
config_files = ["eslint.config.js"]   # edits to these are reported as loosenings
format = "eslint-json"  # auto, sarif, eslint-json, mix, gcc or text
```

`{files}` in a commit-stage command is replaced with the changed files that
match the tool's globs. Only use it with tools that accept file paths.

```sh
qgate baseline create      # record today's violations in qgate.baseline
git add qgate.toml qgate.baseline .gitattributes .claude/skills/qgate .agents/skills/qgate
git commit -m "Add qgate"
```

Tell the developer to review `qgate.toml` and `qgate.baseline`. Every clone needs
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
| `qgate baseline create [--force] [--dry-run]` | Record current violations. |
| `qgate baseline prune [--dry-run]` | Shrink the baseline after fixes. |
| `qgate rules [ID] [--family NAME]` | List rules with their kind, level and rationale. |
| `qgate --skill` | Print this skill for the installed version. |
| `qgate usage` | Print the whole interface as a Usage spec (usage.jdx.dev). |

Every command has `--help` with examples.
