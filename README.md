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

Prebuilt binaries cover Linux (x86_64, aarch64; static, no glibc needed) and
macOS (Intel, Apple Silicon):

```sh
curl -fsSL https://github.com/robertguss/qgate/releases/latest/download/install.sh | sh
```

The script verifies the checksum and installs to `~/.local/bin`. Set
`QGATE_VERSION=v0.1.0` to pin a release or `QGATE_INSTALL_DIR` to change the
location. Archives are also on the
[releases page](https://github.com/robertguss/qgate/releases).

To build from source instead:

```sh
cargo install --locked --git https://github.com/robertguss/qgate --tag v0.1.0 qgate
```

## Releasing

Bump `version` in `Cargo.toml`, merge to `main`, then either push a matching tag
(`git tag v0.2.0 && git push origin v0.2.0`) or run the Release workflow with
that tag (`gh workflow run release.yml -f tag=v0.2.0`). The workflow tests,
builds all four targets and publishes them with `install.sh` to a GitHub
Release.

## Set up a repo

Run these in your own terminal, not through an agent:

```sh
cd your-repo
qgate install            # hooks, merge driver, starter qgate.toml, Claude Code hook (.claude/settings.local.json)
$EDITOR qgate.toml       # set limits and declare your tools (see qgate.toml.example)
qgate baseline create    # record today's violations in qgate.baseline (uses your edited qgate.toml, staged or not)
git add qgate.toml qgate.baseline .gitattributes
qgate approve            # approve the new config and baseline
git commit -m "Add qgate"
```

From then on, `git commit` runs `qgate run --stage commit` and `git push` runs
`qgate run --stage push`.

`install` refuses when another tool already owns the hooks, such as husky or a
custom `core.hooksPath`. Use `qgate install --chain` to keep those hooks and run
them after qgate. Run `install` again after moving the binary, because the hooks
call it by absolute path. Every clone runs its own `qgate install`, because the
git hooks and the Claude Code hook in `.claude/settings.local.json` are local to
the machine.

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

A tool that exits non-zero always fails the run. Its parsed findings in your
own code are errors; findings in dependency or build directories (`deps/`,
`node_modules/`, `_build/`, `target/` and the like) or outside the repo are
only warnings. When nothing parsed is an error in your code, qgate also reports
the tool's failure with the tail of its output, so the real cause is never
hidden behind dependency noise.

## Run in CI

Cloud agents work in fresh clones with no hooks installed, so run the push
stage as a CI job too:

```yaml
- uses: actions/checkout@v4
  with:
    fetch-depth: 0          # qgate needs the merge base with the target branch
- run: curl -fsSL https://github.com/robertguss/qgate/releases/latest/download/install.sh | sh
- run: qgate run --stage push --base origin/${{ github.base_ref || 'main' }} --format github
```

`--base <ref>` checks `HEAD` against its merge base with that ref, so a PR is
judged only on what it changes. `--format github` prints one annotation per
finding, followed by the usual text report, and appends a summary table to
`$GITHUB_STEP_SUMMARY`. On a shallow clone with no merge base, qgate exits 2
and asks for full history instead of comparing against an empty tree. To try
qgate out without blocking merges, set `continue-on-error: true` on the step.
qgate itself has no flag that turns failures off.

Approvals live in each clone's `.git/qgate/`, so CI can't see them yet. A PR
whose change needs approval fails in CI until committed approvals ship.

## Fixtures, test files and C macros

- `fixtures = ["tests/golden/**"]` marks test data. Files there skip the
  built-in checks, and adding new ones needs no approval. Source files that
  qgate parses for functions (`.c`, `.py`, `.rs`, and so on) are still checked
  like any other code under a fixture path, so real code can't hide there.
  Adding a fixture glob needs approval.
- `[[package]]` takes a `glob` instead of a `path` to give files their own
  limits, for example `glob = "tests/**"` with `file_lines = 3000`. A matching
  glob wins over directory packages, and the last matching glob wins.
- `attribute_macros = ["MUST_USE"]` blanks those C/C++ macros, plus any
  `(...)` arguments, before parsing, so they don't cause syntax errors. Line
  numbers are unchanged. Adding a macro needs approval.
- When the parser recovers from small single-line syntax errors, function
  lengths are still right. qgate then prints one summary note per run instead
  of a warning for each file. A per-file `parse-error` warning means an error
  spans several lines and may have hidden a function.

## The baseline

`qgate.baseline` is a sorted text file with one entry per existing violation:

- Function entries are keyed by path and qualified name.
- File entries are keyed by path.
- Duplicate-block entries are keyed by a hash of the shared code, so moving a
  copy keeps its key.

The ratchet works like this:

- A baselined violation may not grow. Once it shrinks, it is held to its new
  size immediately, even before anyone prunes.
- `[ratchet] headroom_lines = 5` lets a baselined function or file grow up
  to 5 lines past its recorded size without approval. The headroom is
  measured from the recorded size, which never rises, so it doesn't add up
  across changes. Raising it needs approval. The default is 0.
- `qgate baseline prune` lowers or removes entries that shrank or were fixed,
  and follows renamed files and moved functions. It never adds or raises an
  entry, so its output needs no approval. Use `--dry-run` to preview.
- `qgate install` registers a merge driver that resolves baseline conflicts by
  keeping the lower value.

## Approvals

A run fails with `unapproved-change` when the change does any of these:

- Adds or raises baseline entries.
- Raises a limit or the ratchet headroom, disables duplication, or adds an `exclude` pattern, a
  `fixtures` glob or an `attribute_macros` entry.
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
