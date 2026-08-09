---
name: land
description: Land the current feature branch — push it, open a PR, squash-merge into main, return to main, and delete the branch. Use when the user asks to "land", "ship", or "merge" the branch they are on.
---

# Land the current branch

Push the branch you are on, open a pull request, squash-merge it into the
default branch, switch back to that branch, pull the merged result, and delete
the feature branch locally and on the remote.

**Argument:** `$ARGUMENTS` — optional PR title. If empty, derive a short title
from the branch's work (the spec title if `.claude/specs/<branch>/spec.md`
exists, otherwise the branch name humanized).

## 1. Preconditions — stop and tell the user if any fails

```bash
ROOT=$(git rev-parse --show-toplevel)
BRANCH=$(git rev-parse --abbrev-ref HEAD)
DEFAULT=$(gh repo view --json defaultBranchRef -q .defaultBranchRef.name)
```

- The working tree must be clean (`git status --short` empty). Do not stash or
  commit for the user; report what is dirty and stop.
- `$BRANCH` must not be `$DEFAULT` — there is nothing to land from the default
  branch.
- `just check` must pass. Run it; if it fails, show the failure and stop —
  never land red.

## 2. Push and open the PR

```bash
git push -u origin "$BRANCH"
```

Write the PR body yourself from the branch's actual content — `git log
$DEFAULT..HEAD` and the diff stat, plus the spec at
`.claude/specs/<branch>/spec.md` if one exists. A `## Summary` bullet list of
what the feature does (not a commit-by-commit recap), then a `## Test plan`
naming what was verified (`just check`, `just cov`, review rounds if a
build-spec run happened). End the body with the Claude Code attribution footer
required by this environment.

```bash
gh pr create --title "<title>" --body "<body>"
```

If a PR for this branch already exists, `gh pr create` fails — use the
existing PR instead of erroring out.

## 3. Squash-merge, sync, and clean up

```bash
gh pr merge --squash --delete-branch=false
git checkout "$DEFAULT"
git pull
git branch -D "$BRANCH"
git push origin --delete "$BRANCH"
```

Notes:

- `--delete-branch=false` on the merge, then delete both refs explicitly after
  the pull confirms the squash commit is on `$DEFAULT`. Deleting the local
  branch needs `-D`, not `-d`: a squash merge leaves the branch's own commits
  unreachable from `$DEFAULT`, so git considers it unmerged.
- If the remote branch was already removed (repo auto-delete on merge), the
  `push --delete` fails harmlessly — say so and move on.
- If the merge itself fails (checks pending, review required, conflicts),
  report the reason and stop; do not force, admin-merge, or rebase without
  being asked.

## 4. Report

Tell the user: the PR URL, the squash commit now at the tip of `$DEFAULT`
(hash and title), and that the feature branch is gone locally and remotely.
