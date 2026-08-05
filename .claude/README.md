# Spec-driven feature workflow

Two commands, four agents. The idea is that context is erased between stages so no
stage inherits the previous one's assumptions, while the specification stays fixed as
the single reference every stage is measured against.

```
/spec <informal description>      interview → .claude/specs/<slug>/spec.md
/build-spec <slug> [--rounds N]   implement → (review → plan → refactor)* → report
```

## Stages

| Stage | Agent | Sees | Writes |
| --- | --- | --- | --- |
| Interview | *(main session)* | you, the codebase | `spec.md` |
| Implement | `spec-implementer` | spec, codebase | source, tests, debugger |
| Review | `spec-reviewer` | spec, decisions, full diff vs base | `review-N.md` |
| Plan | `spec-planner` | spec, review, decisions, code | `plan-N.md`, `decisions.md` |
| Refactor | `spec-refactorer` | spec, plan | source |

Each stage is a fresh subagent. Note what the refactorer does *not* see: the review.
It gets the plan only, so the reviewer's framing cannot ride along into the code. And
every review reads the whole diff against the branch point rather than the last round's
delta, so rounds converge on the spec instead of on each other.

## Why the pieces are shaped this way

- **Requirement IDs (`R1`, `R2`, …)** — every review finding must cite one, or an
  objective defect it can reproduce. This is what stops a reviewer from generating
  infinite taste-based work and keeps the loop terminating.
- **`decisions.md`** — findings the planner rules out of scope, with reasons. The
  reviewer reads it and may not raise them again. Without a ledger, fresh eyes
  re-litigate the same point every round.
- **Out of scope / Deferred sections** — binding on the reviewer, not decoration.
- **The orchestrator stays ignorant** — `/build-spec` never reads code, diffs, or
  reports. It is the only continuous context in the run, so it is the only thing that
  could leak assumptions across the erasure boundaries.

## Loop termination

Exits on the first of: zero blockers and zero major findings; the round cap (default 4);
two consecutive rounds with identical verdict and counts (a stall, which usually means
the spec is ambiguous on the contested point); or a planner that dropped every finding.

Capped and stalled runs are reported as unfinished, not as success.

## Artifacts and isolation

`/build-spec` runs in its own git worktree on its own branch, so a bad round never
touches the working copy. Agents write inside the worktree; the orchestrator copies
artifacts back to `.claude/specs/<slug>/` in the main checkout after every stage, since
that directory outlives the worktree. Nothing under `.claude/` is committed.

Landing the branch is left to you — the loop never merges or pushes.
