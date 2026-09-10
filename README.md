# Ruddy

Ruddy is an early-stage functional language with structural types, type inference, algebraic effects, pattern matching, modules, and a JavaScript backend. This repository contains the compiler, CLI, standard library, browser debugger, language server, and tree-sitter grammar.

## Quick start

Requires a Rust toolchain and [`just`](https://github.com/casey/just). JavaScript projects also require Node.js.

```sh
just install
ruddy new hello
cd hello
ruddy check
ruddy build
ruddy run
ruddy fmt
```

Use `ruddy --help` to see all CLI commands. `ruddy fmt` rewrites a bundle's
sources in one canonical style (`--check` only reports what would change,
`--stdin` formats one file from standard input); a file with syntax errors is
formatted around them, and they are reported. The standard library is installed under `$RUDDY_HOME/std`, or `~/.ruddy/std` when `RUDDY_HOME` is unset.

The repository's `Ruddy.toml` defines the standard-library bundle, with `std/lib.rud` as its root source. Run `ruddy check` from the repository root to check it. The installer copies this manifest and the `std/` source directory into the installed bundle.

## Editor support

The language server is `ruddy lsp`. Configure your editor to launch that
command over stdio, with the directory containing `Ruddy.toml` as its workspace
root. It provides diagnostics, hover, completion, go-to-definition, and
document formatting for `.rud` files. Unsaved local dependency buffers participate in analysis; Git dependencies
and installed std retain source navigation.

For Helix, add the server table and merge these keys into the existing Ruddy
language entry from `just helix` ([Helix configuration](https://docs.helix-editor.com/languages.html)):

```toml
[language-server.ruddy]
command = "ruddy"
args = ["lsp"]

[[language]]
name = "ruddy"
roots = ["Ruddy.toml"]
language-servers = ["ruddy"]
```

The server analyzes the current buffer with recovery, prioritizes active-file
checks, and completes project/backend checks while idle. It tracks source and
manifest changes on disk, including missing module candidates, with a 500 ms poll
of the files the loader actually requested. `Ruddy.toml` selects the root build
configuration. See [the architecture and benchmark notes](docs/query-architecture.md)
for query boundaries and reproducible performance measurements.

## Programs and libraries

Every `Ruddy.toml` declares `kind = "executable"` or `kind = "library"`.
Only libraries can be dependencies. The optional `target` defaults to `js`
for executables and `artifact` for libraries. `artifact` writes the portable
compiler artifact without running a backend; `js` writes both the artifact
and Node.js ESM. A library targeting JS exports its values without calling
an entry point. The optional `platform` names where the output runs, `node`
(the default) or `web`. It decides what `@if` guards see and which cache entry
a dependency gets. A library builds for either platform; an executable for the
web is refused until the JavaScript backend has a web entry adapter, since
today's entry adapter writes to Node's streams and exits its process.

Executables define `main`, callable with `()` and returning a value compatible
with `()` (including a function that never returns). Its effects must be
closed. Both `ruddy check` and `ruddy build` validate that contract; JS builds
also check that Node supports every escaping effect. An artifact-only
executable defers that platform check. Top-level initialization in either
kind must have no escaping effects.

```toml
name = "hello"
version = "0.1.0"
kind = "executable"
root = "main.rud"
target = "js"

[dependencies]
```

```text
let main = fn _ => do
  _ = std::console::print "Hello!"
  return ()
end
```

`_ = expr` is shorthand for `let _ = expr`: it evaluates the expression and
discards its result. Both forms work at file level, inside inline modules,
and in `do` blocks, with the same effect rules. The shortcut also accepts
type annotations (`_ : Nat = expr`) and attributes wherever `let` does.
The formatter preserves either spelling.

Generated executable JS calls `main` once under the Node runtime handlers.
Run it with `ruddy run` or directly with Node. `run` requires an executable
targeting JS; `[run].js` can configure the launch command.

The runtime handles `std::Console.write` and `write_error` (`String -> ()`),
which write to stdout and stderr without adding a newline, and
`std::Process.exit` (`Nat -> Never`). The corresponding `console::print` and
`print_error` helpers append a newline. `process::exit` saturates its code to
255, never resumes, and drains pending console output before termination.
Normal return exits successfully.

The Node runtime also handles `std::FileSystem`, exposed through `std::fs`.
It provides whole-file UTF-8 and binary reads, writes and appends, existence checks,
directory listings and creation, metadata, file and empty-directory removal,
rename, and copy. Operations may suspend and return `Result` values; callers
need no `await`. See [the filesystem module](docs/fs.md) for its signatures
and error behavior.

Local effect handlers can intercept all these operations. Platform effects
are recognized by their complete structural identity, independently of where
they were declared. Std can still be overridden or disabled with `std = false`;
a pure executable needs no std dependency.

When a JavaScript root bundle is built, its public values receive host-callable
adapters. On Node, exported functions run under the Console, Process, and
FileSystem handlers, including aliases imported from std. Both `check` and
`build` reject exports whose effects cannot be handled. Dependency artifacts,
private definitions, and internal Ruddy calls retain their effect interfaces;
local handlers can still intercept them. See [host exports](docs/cps.md#library-exports-and-initialization).

A struct literal may spread one value's fields into itself with `..`, written
last, as in `{ x: 1, ..base }`: the result has every field of `base` and the
fields named before the `..`, which replace fields of the same name at
whatever type they have, so `fn v => { x: 1, ..v }` updates or extends
whatever struct it is given while keeping the rest of its fields.

Any top-level definition may carry metadata: attributes written in front of
it as `@key` or `@key <literal>`, where the literal is a string, number,
boolean, tag, or a tuple, array, or struct of those, as in
`@deprecated "use nat::add" @since 2n let add = ...`. The compiler refuses a
repeated key, carries metadata through compilation, and shows it in the debugger
and bundle artifact.

`@private` makes a definition accessible only within its declaring bundle.
It accepts only unit (`@private`, `@private ()`, or `@private {}`). It applies
to values, externs, types, effects, and modules; a private module hides every
definition beneath it, including in module files. A pattern binding marks every
name it binds private. Unmarked definitions are public unless enclosed by a
private module. An executable's root `main` must be public.

Public definitions may alias private definitions, and public signatures may
mention private types or effects, explicitly or through inference: their
structural meaning remains available to callers. Private names are absent from
dependent bundles' source lookup and JavaScript exports; private implementation
code still runs when needed.

`@if` compiles a definition only for the builds its conditions name. Its value
is a struct whose fields are facts of the root project's build, each a string:
`target` (`"js"` or `"artifact"`) and `platform` (`"node"` or `"web"`). Every
fact named must hold; a fact not named may be anything. The facts are the root
project's, so a library's guard sees the build of the executable depending on
it. A definition whose guard does not hold is dropped before names are
resolved: a guarded `module` whose file is missing costs nothing, and two
definitions of one name guarded for two builds do not collide. A value no
build has holds for no build. An unknown fact, a missing condition, or a value
that is not a string is refused.

```text
@if {target: "js"} module js
@if {target: "js", platform: "node"} let now = js::node_now
@if {target: "js", platform: "web"} let now = js::web_now
@if {target: "artifact"} let now = fn _ => 0n
```

`@ffi` declares foreign completion and callback protocols. `@export "sync"`
and `@export "promise"` select fixed JavaScript export contracts. Ruddy code
uses the same function types for immediate and delayed completion; see
[CPS execution and foreign completion](docs/cps.md) for the boundary contracts.

All other metadata keys remain uninterpreted.

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

See [Numbers](docs/numbers.md) for target-sized and fixed-width integer types, literals, and arithmetic.
