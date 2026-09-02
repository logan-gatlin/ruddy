# Repository instructions

## Tests

- Run the Rust test suite only through `just test`.
- Never invoke `cargo test` directly, including for focused tests or with additional test arguments.
- The `just test` recipe places the complete test process tree in a memory-limited systemd scope. Bypassing it can exhaust system memory and terminate the coding-agent session.

## Agent skills

### Issue tracker

Issues and specs are tracked as local Markdown under `.scratch/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Triage uses the five default canonical label strings. See `docs/agents/triage-labels.md`.

### Domain docs

Domain documentation uses a single-context layout. See `docs/agents/domain.md`.
