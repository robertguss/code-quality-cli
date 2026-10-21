# qgate

A local, deterministic quality gate for code written by AI agents. It runs as
git hooks on every commit and push, so the mechanical checks that usually wait
on remote CI finish on your machine in seconds, and AI review can spend its
tokens on judgment instead.

- **One gate, one result.** Your repo's own linters, type checkers and tests run
  in parallel alongside qgate's built-in checks, and every run ends in a single
  pass or fail.
- **Built-in checks in any language.** Function length, file length and
  duplicated code across the whole repo. Function lengths use 14 bundled
  tree-sitter grammars: Rust, Python, JavaScript, TypeScript/TSX, Go, Java, C,
  C++, Ruby, Bash, C#, PHP and Elixir. Every other text file still gets the
  file-length check.
- **Elixir:** each `def`, `defp`, `defmacro` and `defguard` clause is measured
  on its own, named `Module::fun`, and so is each ExUnit `test` block, named by
  its string inside any `describe`. A definition with a computed name, such as
  `def unquote(name)()`, is measured as `<dynamic>`, so metaprogramming can't
  hide it.
- **A ratchet, not a cliff.** Existing violations go in a baseline that can only
  shrink, so a legacy repo passes on day one and new or worse code fails.
- **Agents can't skip the gate, and loosening it is never silent.** A Claude
  Code hook blocks `--no-verify`, hook-skipping git config and tampering with
  the hooks (see [The agent hook](#the-agent-hook)). Raising a limit,
  growing the baseline, editing a linter config, excluding files or adding an
  `eslint-disable` is allowed, but each one shows as a `loosening` warning in
  the run output.
- **Built for agents to call.** Nothing ever prompts. Every command has
  `--help` with examples and supports `--format json`. Every failure prints the
  command that resolves it. `qgate --skill` prints an agent skill that explains
  the gate, and `qgate usage` prints the whole interface as a
  [Usage spec](https://usage.jdx.dev). Exit codes are `0` pass, `1` findings,
  `2` usage or config error.

## Install

Prebuilt binaries cover Linux (x86_64, aarch64; static, no glibc needed) and
macOS (Intel, Apple Silicon):

```sh
curl -fsSL https://github.com/robertguss/qgate/releases/latest/download/install.sh | sh
```

The script verifies the checksum and installs to `~/.local/bin`. Set
`QGATE_VERSION` to pin a release or `QGATE_INSTALL_DIR` to change the location,
for example `curl -fsSL ... | QGATE_VERSION=v0.3.0 sh`. Archives are also on the
[releases page](https://github.com/robertguss/qgate/releases).

To build from source instead, using the tag of the release you want:

```sh
cargo install --locked --git https://github.com/robertguss/qgate --tag v0.3.0 qgate
```

Each repo's `qgate.toml` pins the qgate version its baseline was measured with
(`qgate_version`). A different binary refuses to run there, exits 2 and prints
the command that installs the pinned version.

### Updating

`qgate update` is new in v0.4.0. On v0.3.0 or earlier, run `install.sh` once
more to get it.

```sh
qgate update --check   # is a newer release out? (add --format json for agents)
qgate update           # install it over the running binary
qgate update --tag v0.2.0   # install a specific release, newer or older
```

`update` downloads the same archive `install.sh` would, verifies its checksum,
runs the new binary once to confirm it works here, then swaps it in place, so
the git hooks (which call qgate by absolute path) keep working. It needs `curl`
and `tar`.

Then upgrade each repo, because its `qgate.toml` still pins the old version and
qgate refuses to run until it matches:

```sh
$EDITOR qgate.toml        # qgate_version = "<new version>"
qgate install             # rewrites the committed agent skill for the new release
git add qgate.toml .claude/skills/qgate .agents/skills/qgate
git commit -m "Upgrade qgate"
```

That commit shows a `qgate_version` loosening warning, which is expected. Check
`docs/releases/<tag>.md` for any other upgrade steps, such as removing the
`[approvals]` section when moving to v0.4.0.

## Set up a repo

```sh
cd your-repo
qgate install            # hooks, merge driver, starter qgate.toml (with Mix tools in a Mix project), Claude Code hook, agent skill
$EDITOR qgate.toml       # set limits and declare your tools (see qgate.toml.example)
qgate baseline create    # record today's violations in qgate.baseline (uses your edited qgate.toml, staged or not)
git add qgate.toml qgate.baseline .gitattributes .claude/skills/qgate .agents/skills/qgate
git commit -m "Add qgate"
```

From then on, `git commit` runs `qgate run --stage commit` and `git push` runs
`qgate run --stage push`.

`install` refuses when another tool already owns the hooks, such as husky or a
custom `core.hooksPath`. Use `qgate install --chain` to keep those hooks and run
them after qgate. Run `install` again after moving the binary, because the hooks
call it by absolute path. Every clone runs its own `qgate install`, because the
git hooks and the Claude Code hook in `.claude/settings.local.json` are local to
the machine. `qgate install --dry-run` shows what it would write.

## Configuration

Everything lives in `qgate.toml`. [`qgate.toml.example`](qgate.toml.example)
documents every key; unknown keys are an error.

| Key | Default | What it does |
|---|---|---|
| `qgate_version` | set by `install` | The release this repo is pinned to |
| `[limits] function_lines` | 50 | Longest function, in physical lines |
| `[limits] file_lines` | 500 | Longest file |
| `[ratchet] headroom_lines` | 0 | How far a baselined function or file may grow |
| `[duplication] enabled`, `min_tokens` | `true`, 50 | Duplicated-block detection and its smallest block |
| `exclude` | none | Extra globs skipped by the built-in checks |
| `fixtures` | none | Test data globs (see below) |
| `attribute_macros` | none | C/C++ macros blanked before parsing |
| `ignore_patterns` | none | Extra inline-ignore markers to warn about |
| `[[tool]]` | none | Your repo's own tools: `name`, `command`, `globs`, `stage`, `timeout_secs`, `config_files`, `format` |
| `[[package]]` | none | Per-directory (`path`) or per-glob (`glob`) `function_lines` and `file_lines` |

A tool's `command` gets the matching changed files in place of `{files}`; a
command without `{files}` runs once on the whole project. `stage` is `commit`
(the default, which also runs at push) or `push`. Binary files, lockfiles,
`*.min.*`, `vendor/`, `node_modules/`, `dist/` and files marked
`linguist-generated` or `linguist-vendored` are always excluded.

## The agent skill

`qgate install` writes a skill to `.claude/skills/qgate/SKILL.md` (Claude Code)
and `.agents/skills/qgate/SKILL.md` (Codex and other agents that read
`.agents/skills`). Commit both. An agent that opens the repo then knows how to
read findings, fix them without loosening the gate, shrink the baseline, and set
qgate up, before its first commit is blocked. `qgate --skill` prints the same
file, matched to the installed version, for agents without a skill system.

Every run checks the committed copies:

- A copy written by an older qgate is a `skill-outdated` warning that names
  both versions. `qgate install` updates it.
- Any other difference, such as a hand edit, is a `skill-edited` error, so an
  agent can't rewrite the instructions later agents read. `qgate install`
  restores it. A copy written by a newer qgate also fails here; update qgate.
- A missing copy is a `skill-missing` warning.

## What runs when

| | Commit | Push |
|---|---|---|
| Content checked | The staged index, never unstaged edits | The tip of each pushed ref |
| Compared against | `HEAD` | The merge base with the remote default branch |
| Built-in checks | Changed files. Duplication is found repo-wide and reported only when a copy touches the change | Same, over the whole push |
| Repo tools | `stage = "commit"` tools whose globs match changed files | Every tool |
| Guard | The staged change | Each pushed commit against its first parent |

Repo tools run in the working tree when it matches what is staged. Otherwise,
and always at push, they run in qgate's own checkout at `.git/qgate/checkout`.
qgate never stashes, never re-stages and never edits your files.

The checkout is kept between runs and updated in place, so only changed files
are rewritten and builds inside it stay incremental. Installed dependencies
(`node_modules`, `.venv`, `deps` and the like) are linked in from your working
tree. Build output (`_build`, `target`, `.gradle`) stays in the checkout, so a
tool run never writes into your own build or waits on its lock. The cost is a
second build directory on disk and a slow first run. Runs that need the
checkout take turns. To reclaim the space, delete `.git/qgate/checkout`; the
next run rebuilds it.

When you run a hook from a terminal, qgate prints progress to stderr: what it
is checking, which tools are running and for how long, and each one's result
and time as it finishes. The report itself still goes to stdout. Progress is
off when stderr is not a terminal (agents, CI, pipes) and with `--format json`
or `--format github`. `QGATE_PROGRESS=1` turns it on anyway, without colour or
the live status line, and `QGATE_PROGRESS=0` turns it off. `NO_COLOR` is
honoured.

A tool that exits non-zero always fails the run. Its parsed findings in your
own code are errors; findings in dependency or build directories (`deps/`,
`node_modules/`, `_build/`, `target/` and the like) or outside the repo are
only warnings. When nothing parsed is an error in your code, qgate also reports
the tool's failure with the tail of its output, so the real cause is never
hidden behind dependency noise.

A tool's `format` says how to read its output: `sarif`, `eslint-json`, `mix`
(Elixir compile diagnostics, ExUnit failures and `mix format
--check-formatted`), `gcc` (`path:line: message` lines) or `text`. The default,
`auto`, tries them in that order. Colour codes are stripped first. Credo
reports through SARIF: `mix credo --format sarif`.

## Run in CI

Cloud agents work in fresh clones with no hooks installed, so run the push
stage as a CI job too:

```yaml
- uses: actions/checkout@v4
  with:
    fetch-depth: 0          # qgate needs the merge base with the target branch
- name: Install the qgate version pinned in qgate.toml
  run: |
    v=$(sed -n 's/^qgate_version *= *"\(.*\)"/\1/p' qgate.toml)
    curl -fsSL https://github.com/robertguss/qgate/releases/latest/download/install.sh | QGATE_VERSION="v$v" sh
    echo "$HOME/.local/bin" >> "$GITHUB_PATH"
- run: qgate run --stage push --base origin/${{ github.base_ref || 'main' }} --format github
```

`--base <ref>` checks `HEAD` against its merge base with that ref, so a PR is
judged only on what it changes. `--format github` prints one annotation per
finding, followed by the usual text report, and appends a summary table to
`$GITHUB_STEP_SUMMARY`. On a shallow clone with no merge base, qgate exits 2
and asks for full history instead of comparing against an empty tree. To try
qgate out without blocking merges, set `continue-on-error: true` on the step.
qgate itself has no flag that turns failures off.

## Fixtures, test files and C macros

- `fixtures = ["tests/golden/**"]` marks test data. Files there skip the
  built-in checks, and adding new ones is not a loosening. Source files that
  qgate parses for functions (`.c`, `.py`, `.rs`, and so on) are still checked
  like any other code under a fixture path, so real code can't hide there.
  Adding a fixture glob is a loosening.
- `[[package]]` takes a `glob` instead of a `path` to give files their own
  limits, for example `glob = "tests/**"` with `file_lines = 3000`. A matching
  glob wins over directory packages, and the last matching glob wins.
- `attribute_macros = ["MUST_USE"]` blanks those C/C++ macros, plus any
  `(...)` arguments, before parsing, so they don't cause syntax errors. Line
  numbers are unchanged. Adding a macro is a loosening.
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
  to 5 lines past its recorded size. The headroom is measured from the
  recorded size, which never rises, so it doesn't add up across changes.
  Raising it is a loosening. The default is 0.
- `qgate baseline prune` lowers or removes entries that shrank or were fixed,
  and follows renamed files and moved functions. It never adds or raises an
  entry, so its output is never a loosening. Use `--dry-run` to preview.
- `qgate install` registers a merge driver that resolves baseline conflicts by
  keeping the lower value.

## Loosenings

A run warns with `loosening` when the change does any of these. The warning
never fails the run; it is there so you, and the agent, see every place the
gate got weaker:

- Adds or raises baseline entries.
- Raises a limit or the ratchet headroom, disables duplication, or adds an `exclude` pattern, a
  `fixtures` glob or an `attribute_macros` entry.
- Changes or removes a declared tool.
- Edits a tool's config file. This includes any file with the same name
  elsewhere in the repo, such as a nested `.eslintrc.json`.
- Edits `package.json` scripts or a `pyproject.toml` `[tool]` section.
- Elixir: edits `mix.exs` aliases, `cli`, or the `project` keys that change
  what gets compiled, tested or covered (dependency bumps don't count), or
  edits `.formatter.exs`, `.credo.exs`, `.dialyzer_ignore.exs`,
  `coveralls.json` or `test_helper.exs`.
- Edits a Makefile or justfile when a tool runs `make` or `just`.
- Marks files `linguist-generated` or `linguist-vendored`, changes a merge
  driver, or adds files under excluded paths. Go files with a
  `// Code generated ... DO NOT EDIT.` header are skipped wherever they are,
  so adding one, or adding the header to an existing file, is a loosening.
  Regenerating or moving a generated file is not.
- Adds an inline ignore such as `eslint-disable`, `@ts-expect-error`, `# noqa`,
  `#[allow(...)]`, `//nolint`, `credo:disable` or `@tag skip:`, or one of your
  `ignore_patterns`.

When a change has more than five loosenings of one kind, such as many new
files under an excluded path, qgate shows them as one line with a count.

## The agent hook

`qgate install` adds a Claude Code `PreToolUse` hook to
`.claude/settings.local.json` that runs `qgate agent-hook` before each shell
command and file edit. It blocks:

- `--no-verify` (and its abbreviations) on any git command, and `git commit -n`.
- Git config that skips or redirects hooks, such as `-c core.hooksPath=...` or
  `git config core.hooksPath`.
- `git commit-tree`, `git update-ref` and `git replace`, which write commits or
  refs without running hooks.
- Writing, moving or deleting the git hooks, `.git/config`, `.git/info` or the
  project and user `.claude/settings*.json` files.
- Commands it can't verify as safe, such as git aliases it can't expand.

Each block says why and tells the agent to fix the findings and commit again.

## Known limits

qgate stops agent shortcuts. It does not stop a determined human or every
possible agent:

- Anyone with a shell can delete the hooks. A Claude Code setting can also turn
  off non-managed hooks.
- The agent hook covers Claude Code only. It inspects commands the agent runs
  directly. Commands run indirectly are not inspected, such as a script file, a
  `make` or `npm run` target, or an interpreter running code built at runtime.
- Rebase, cherry-pick, revert, `git am` and clean merges do not run
  pre-commit. The push hook is the backstop for those.
- Clones without `qgate install` are not gated.
- husky regenerates its hook directory on `npm install`, so re-run
  `qgate install --chain` afterwards.
- The repo's own tools must already be installed. qgate runs them but does not
  install them.

## Developing qgate

Before opening a PR, run:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

### Releasing

Bump `version` in `Cargo.toml` and `qgate_version` in `qgate.toml.example`,
write the release notes in `docs/releases/<tag>.md` (they appear above the
generated changelog), and merge to `main`. Then either push a matching tag
(`git tag v0.4.0 && git push origin v0.4.0`) or run the Release workflow with
that tag (`gh workflow run release.yml -f tag=v0.4.0`). The workflow tests,
builds all four targets and publishes them with `install.sh` to a GitHub
Release.

`skill/history.txt` lists the digest of every released version's agent skill,
so `qgate run` can tell an outdated copy from an edited one. `cargo test` fails
until the line for the current version matches; it prints the line to use.
Replace that line while the version is unreleased, and never change the lines
of released versions.
