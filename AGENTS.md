# Agent notes for qgate

## Project tracking

Tasks, decisions, the changelog and status live in the qgate hub in Robert's Notion (Dev Hub > Projects > qgate). Task IDs use the prefix `QG`, and PR titles start with the task ID, as in `QG-07: Add a Mix and ExUnit output parser`.

Use the `qgate-notion-sync` skill in `.claude/skills/` to keep the hub current when a PR opens or merges, when Robert makes a decision, or when a release ships.

## Done means

`cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` all pass. The repo has no CI on pull requests, so report these results in the PR description.
