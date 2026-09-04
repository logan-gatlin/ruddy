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

A struct literal may spread one value's fields into itself with `..`, written
last, as in `{ x: 1, ..base }`: the result has every field of `base` and the
fields named before the `..`, which replace fields of the same name at
whatever type they have, so `fn v => { x: 1, ..v }` updates or extends
whatever struct it is given while keeping the rest of its fields.

Immutable homogeneous arrays use `[value, ...]` literals and `[Type]` types.
A literal may spread other arrays into place with `..`, as in `[..a, x, ..b]`,
and a `match` may take an array apart by length with patterns such as `[]`,
`[first, ..rest]`, and `[first, .., last]`, where the one `..` binds the
elements between the named ones. The `std::array` module provides `len`,
safe `get`, `set`, `slice`, and `pop`, and persistent `push`, `prepend`, and
`concat`; every update returns a new array without changing its inputs, and
slicing and concatenation take logarithmic time.

A function that immediately matches its sole argument may omit the argument
and match wrapper: `fn | #Some value => value | #None => 0n` is shorthand for
an ordinary unary function whose body matches that implicit argument. The
leading `|` is required, and parentheses delimit a nested shorthand when an
enclosing arm follows it.

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
