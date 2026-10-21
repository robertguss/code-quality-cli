---
name: qgate-notion-sync
description: Keep the qgate project hub in Robert's Notion (Dev Hub) in sync with GitHub. Use when a qgate PR opens or merges, when Robert makes a decision, when a release ships, or when asked to update the hub, status or changelog.
---

# Sync the qgate hub in Notion

The qgate hub lives in Robert's personal Notion under Dev Hub > Projects > qgate. It tracks tasks, decisions, the changelog and status for this repo. GitHub is the source of truth for code; the hub summarizes it.

## Before you change anything

1. Fetch the hub's **For agents** page (Notion ID `19d700320c4583e9b8e58112902faeab`). It maps every page and data source ID and has the full sync rules. Follow it over this file if they differ.
2. Read live GitHub state for the PRs involved. Memory and earlier summaries are leads, not facts.

## When to update

| Event | What to update |
| --- | --- |
| A PR opens | Set its task card to In progress and fill in PR link. |
| A PR merges | Set the card to Done and add a **Result** (PR, short SHA, date, any deviation from the scope). Tick the Done when boxes that have evidence. Add a Changelog line under today's date. Refresh Status. |
| Robert decides something | Add a Decision Log entry with the next number, the date and his exact words. Never paraphrase his words. |
| A release ships | Add a Changelog line, update the milestone row on Status, and close the release card. |
| New work is planned | Add a task card with the next QG number, using Scope, Files, Done when and Result. |
| An idea is out of scope | Add it to the Idea Parking Lot, not to Tasks. |

After any Status refresh, set **Health** and **Status updated** on qgate's row in the Projects database.

## Conventions

- Task IDs are `QG-NN` (for example `QG-07`). PR titles start with the task ID.
- "You are here" on Status is three sentences at most. History goes on the Changelog.
- Write in plain, full sentences. Don't mention sessions, threads or tool names.
