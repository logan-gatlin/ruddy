# Ruddy

Ruddy is an early-stage functional language with structural types, type inference, algebraic effects, pattern matching, modules, and a JavaScript backend. This repository contains the compiler, CLI, standard library, browser debugger, and tree-sitter grammar.

## Quick start

Requires a Rust toolchain and [`just`](https://github.com/casey/just). JavaScript projects also require Node.js.

```sh
just install
ruddy new hello
cd hello
ruddy check
ruddy build
ruddy run
```

Use `ruddy --help` to see all CLI commands. The standard library is installed under `$RUDDY_HOME/std`, or `~/.ruddy/std` when `RUDDY_HOME` is unset.

## Development

```sh
just build       # build the workspace
just check       # formatting, linting, and tests
just test        # run tests in a memory-limited scope
just dev         # serve the live-reloading debugger on :7878
just grammar     # regenerate and test the tree-sitter parser
just cov         # measure compiler coverage (nightly Rust)
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for project conventions. Run tests through `just test`, not `cargo test`.
