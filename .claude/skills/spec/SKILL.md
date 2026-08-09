---
name: spec
description: Turn an informal feature idea into a precise, testable specification through an interview. Use when the user describes a feature they want built in ruddy, asks to "spec out" or "write a spec for" something, or before running /build-spec. Produces .claude/specs/<slug>/spec.md.
---

# Write a feature specification

You are interviewing the user to convert a vague feature idea into a specification
precise enough that a fresh agent — one that has never seen this conversation — can
implement it correctly, and another fresh agent can judge whether the implementation
is right.

The spec is the only thing that survives the context erasure in `/build-spec`. Every
assumption you leave in your head instead of in the file is an assumption the
implementer will get wrong.

**Argument:** `$ARGUMENTS` — the user's informal description, or an existing slug to
revise. If empty, ask what they want to build.

## 1. Set up

```bash
ROOT=$(git rev-parse --show-toplevel)
```

Specs live at `$ROOT/.claude/specs/<slug>/spec.md`, gitignored, so a spec never reaches
the diff of the feature it describes.

Pick a short kebab-case `<slug>` from the feature name (`sum-types`, `let-polymorphism`,
`span-tracking`). If that directory already exists, you are revising: read the existing
spec and treat this as an amendment pass.

If HEAD is the repository's default branch (`git branch --show-current` says `main`),
check out a new branch named after the slug before writing anything:

```bash
git switch -c <slug>
```

If a branch by that name already exists, switch to it instead. If HEAD is already on a
non-default branch, stay there — the user chose it.

## 2. Ground yourself before asking anything

Read `CLAUDE.md`, then read enough of the codebase to know which phases the
feature touches — `src/token.rs`, `src/parse.rs`, `src/ir.rs`, `src/inference/`,
`src/types.rs`, `src/ui.rs`, and the matching `debug/src/stage/` and `debug/src/print/`
modules. Look at `tests/src/` for how the existing tests are shaped.

Never ask the user something the repository already answers. Questions about existing
naming, current file layout, or how a phase works today are wasted turns — go read it.
Ask only about things that live in the user's head.

## 3. Interview

Use `AskUserQuestion`, batching up to four questions per call, and keep going until the
remaining unknowns would not change a line of code. Two to four rounds is typical.

Aim at decisions where different answers produce materially different implementations:

- **Surface syntax** — concrete source snippets. Get the user to write, or approve, the
  exact text a `.hc` file will contain. Ambiguous grammar is the single most common
  source of a wrong implementation.
- **Semantics and edge cases** — empty cases, nesting, recursion, shadowing, interaction
  with existing features (row polymorphism, type constructors, inference).
- **Error behaviour** — what is rejected, at which phase, and roughly what the diagnostic
  says. ruddy's user-facing prose is plain English, not type-theory jargon; internal names
  stay technical. Capture the wording the user wants.
- **Scope boundaries** — what is explicitly *not* part of this change. Push for these;
  they are what keeps the review loop from expanding forever.
- **Debugger surface** — `CLAUDE.md` requires the debugger to keep up. Which tab shows
  this, what does it render, does it need a new tab.
- **Inference interaction** — for anything touching types: what unifies with what, what
  the solver must now handle, what should stay unconstrained.

Offer a recommended default in each question so the user can move fast, and put the
recommendation first. When the user says "you decide", decide, and record the decision
in the spec as a stated assumption rather than an open question.

Do not write code, and do not start implementing. This skill produces one file.

## 4. Write the spec

Write `$ROOT/.claude/specs/<slug>/spec.md` with this structure. Requirements get stable
IDs — `R1`, `R2`, … — because every downstream review finding must cite one.

```markdown
# <Feature name>

**Slug:** <slug>
**Status:** draft
**Written:** <YYYY-MM-DD from `date`>

## Summary

One to three sentences. What this feature is, in the user's terms.

## Motivation

The problem it solves, and why it is worth doing now.

## Scope

### In scope
- ...

### Out of scope
- ... (explicit non-goals — a reviewer may not raise findings against these)

## Requirements

Each requirement is one testable statement of observable behaviour.

- **R1.** ...
- **R2.** ...

## Syntax and examples

Concrete `.hc` source with the expected result for each — inferred type, IR shape,
printed output, or diagnostic. Include at least one example per requirement that has
a surface syntax.

```
<source>
```
Expected: <result>

## Error cases

| Input | Phase | Diagnostic (plain English) |
| --- | --- | --- |
| ... | parse / inference / … | ... |

## Compiler phases touched

Which of token, parse, ir, inference, types, ui change, and how. Note anything that
must deliberately stay untouched.

## Debugger requirements

What `./debug/` must show. Which tab, what it renders, whether a new tab is needed.
The debugger must compile; per CLAUDE.md the feature is not done otherwise.

## Tests

What must exist in the `ruddy-tests` crate, by module (`tests/src/parse.rs`,
`tests/src/inference.rs`, …). Name the specific cases, including the failure cases.
No `#[cfg(test)]` in `src/` or `debug/src/`.

## Definition of done

- All requirements above are satisfied.
- `just check` passes (fmt, clippy, workspace tests).
- The debugger builds and reflects the feature.
- <anything feature-specific>

## Assumptions

Decisions made on the user's behalf, and what was assumed. An implementer may rely
on these.

## Deferred

Things raised during the interview and deliberately postponed. Out of scope for review.
```

## 5. Hand off

Show the user the spec path and a compressed summary — the requirement list, the scope
boundaries, and anything you had to assume. Ask them to correct anything wrong, since
this file is the only context the implementer gets.

Then tell them: `/build-spec <slug>` runs implementation and the review/refactor loop
from here, each stage in a fresh context.
