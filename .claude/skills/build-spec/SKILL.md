---
name: build-spec
description: Implement a spec from .claude/specs/ and run the review/refactor loop until only minor issues remain. Use after /spec, or when the user asks to build, implement, or ship a spec by slug. Each stage runs in a fresh subagent context with the spec as the only shared reference.
---

# Build a spec

Run a spec to completion: implement it, then review and refactor in rounds until the
reviewer finds nothing above minor severity.

**Argument:** `$ARGUMENTS` — the spec slug, optionally followed by `--rounds N`
(default 4). If no slug is given, list `.claude/specs/*/` and ask which one.

## Your role: dispatcher, not participant

The point of this workflow is that each stage starts from zero. An implementer's
assumptions do not reach the reviewer; a reviewer's phrasing does not reach the
refactorer. You are the only continuous context, so you are the leak, and you plug
it by staying ignorant:

- **Do not read source files, the diff, or any code an agent wrote.** Not to check
  its work, not to summarise it, not out of curiosity.
- **Do not read `review-N.md` or `plan-N.md`.** Agents return short summaries; relay
  those. The files exist for the next agent and for the user, not for you.
- **Do not fix anything yourself**, however small. A failing stage gets a fresh agent,
  not your hands.
- **Do not add your own guidance to agent prompts.** A prompt carries paths, the round
  number, the base commit, and nothing else you invented. If you find yourself typing
  "note that…" into a prompt, that note belongs in the spec instead — stop and ask the
  user whether to amend it.

You may read the spec's title and requirement count for your own report. That is all.

## 1. Set up

```bash
ROOT=$(git rev-parse --show-toplevel)
DIR="$ROOT/.claude/specs/<slug>"
```

The spec of record is `$DIR/spec.md`. Verify it exists; if not, say so and suggest
`/spec`.

Everything runs on the branch already checked out — no worktree, no isolation. Every
agent inherits this working directory, so they share the one tree and the commits land
where the user can see them.

Two preconditions, both worth stopping for:

- **The working tree must be clean.** Every review diffs the whole branch against the
  base commit, so uncommitted changes would be read as the implementation's work. Check
  `git status --short`; if it is not empty, tell the user what is dirty and stop. Do not
  stash — the stash stack is shared with every other checkout and is not yours to move.
- **If HEAD is the repository's default branch**, say so and ask whether to branch first
  before running anything. The loop commits several times; the user may not want that
  history on `main`.

Then record the branch point:

```bash
BASE=$(git rev-parse HEAD)
```

`$DIR/spec.md` is what every agent reads. Never let an agent edit it, and never edit it
yourself mid-run — a spec that moves under the loop makes every round incomparable.

Maintain `$DIR/run.md` as you go: slug, branch, `$BASE`, start time from `date`, and a
line per stage as it finishes. It is the audit trail if the loop is interrupted, so
write each line as the stage returns rather than at the end.

`.claude/specs/` is gitignored, so none of this reaches the diff. If an agent ever
reports committing files under `.claude/`, stop the loop and tell the user.

## 2. Implement

Spawn `spec-implementer` with `run_in_background: false` and wait. The prompt gives it:

- the absolute path to `$DIR/spec.md` — "the specification is the authority; read it in
  full first, and never edit it"
- `$ROOT`, the repository it works in
- `$BASE`
- instruction to commit source changes only, once `just check` is green, on the branch
  already checked out — never branching, merging, or pushing

It returns a short summary: files touched, requirements implemented, anything it could
not do. Relay that to the user in your own words. Do not open the files it names.

## 3. Review / refactor loop

For round `N` from 1 to the round cap:

**a. Review.** Spawn `spec-reviewer`, waiting for it. Prompt gives it `$DIR/spec.md`,
`$DIR/decisions.md` (may not exist yet — that is fine), `$BASE`, the round number, and
the output path `$DIR/review-N.md`.

It returns a verdict line and counts:

```
VERDICT: <clean|minor-only|needs-work>
blockers: <n>  major: <n>  minor: <n>
- <one-line title per finding, with severity and the requirement it cites>
```

**b. Stop conditions.** End the loop when any of these holds:

- `blockers == 0 && major == 0` — success, the intended exit.
- `N` reached the round cap — report as incomplete, not as success.
- Two consecutive rounds returned the same verdict with the same counts — the loop is
  ping-ponging. Stop and report a stall; that usually means the spec is ambiguous on
  the contested point.
- The planner ruled every finding out of scope — nothing to do.

**c. Plan.** Spawn `spec-planner` with `$DIR/spec.md`, `$DIR/review-N.md`,
`$DIR/decisions.md`, and the output path `$DIR/plan-N.md`. It filters findings against
the spec, appends anything it rejects to `decisions.md` with a reason, and writes a
self-contained ordered plan. It returns task and dropped counts.

**d. Refactor.** Spawn `spec-refactorer` with `$DIR/spec.md`, `$DIR/plan-N.md`, and
`$BASE`. Do not give it the review path — the plan is deliberately the only channel,
so the reviewer's framing cannot reach the hands that change the code. It applies the
plan, gets `just check` green, and commits.

Then continue to round `N+1`. Every review reads the whole diff against `$BASE`, not
just the last round's delta, so the spec stays the reference rather than the previous
round's opinion.

Run stages strictly one at a time. They mutate the same tree.

## 4. Report

Write the final state into `run.md`, then tell the user:

- Which round it exited on and why — clean, capped, or stalled.
- Findings per round, as counts, showing the trend.
- Remaining minor issues, by title, from the last reviewer's return summary.
- Anything the planner ruled out of scope, and the reason — these are the spots where
  the spec and the reviewer disagreed, and they are worth the user's attention.
- The branch, and the commits the loop added to it.
- How to land it: review the commits, then merge or push. Do not merge or push
  yourself, and do not reset or rewrite the branch to tidy it.
- Where the artifacts are: `$DIR`.

If the loop exited capped or stalled, say plainly that the feature is not finished and
what is left.
