# Conditional Compilation

Status: implemented

## Problem Statement

A bundle has one source tree for every target it is built for. A library that
wants to offer a JavaScript-specific definition, or an executable that wants a
module only when it is built for one backend, has no way to say so: every
definition in every file is compiled for every build. The Rust backend
investigation (`.scratch/rust-backend/research.md`) names this as one of the
blockers to a second backend, since `std`'s externs are JavaScript source and
a program for another target would still carry them.

## Solution

A definition may be guarded by an `@if` attribute whose value is a struct of
conditions. When every condition holds for the build, the definition is
compiled as if the attribute were any other metadata; when one does not, the
definition is dropped before name resolution, as if it had not been written.

```
@if {target: "js"} module js
@if {target: "js"} let now = js::now
@if {target: "artifact"} let now = fn _ => 0n
```

The conditions are the facts of the root project's build, each a string
compared with what `Ruddy.toml` spells: `target` (`js` or `artifact`) and
`platform` (`node`, the default, or `web`). Target is what the compiler
emits; platform is where the output runs, since the same JavaScript runs
under Node or in a browser and what differs is the host. A guard names only
the facts it cares about; one it does not name may be anything, so
`@if {platform: "web"}` spans both targets. A dependency is judged against
the *root's* build, not its own manifest's, so a library sees the build its
consumer is making. An unknown field is refused so that adding a fact later
cannot silently change what an existing guard means.

## User Stories

1. As a Ruddy programmer, I want `@if {target: "js"}` in front of a definition to include it only in JavaScript builds, so that a backend-specific definition does not reach other targets.
2. As a Ruddy programmer, I want a guard on a file module (`@if {target: "js"} module js`) to skip reading the module's file when the guard does not hold, so that a target's module need not exist for other targets.
3. As a Ruddy programmer, I want two definitions of one name, each guarded for a different target, accepted, so that one bundle can carry a definition per target.
4. As a Ruddy programmer, I want a guard on a definition inside an inline module or a module file to work the same as at the top level, so that the rule has no special cases.
5. As a library author, I want my library's guards judged against the target of the project being built, so that `@if {target: "js"}` in a library is true when a JavaScript executable depends on it.
6. As a library author, I want `artifact` to be a target I can name, so that a library checked on its own can guard a definition for that case.
7. As a Ruddy programmer, I want a target name the compiler does not know to make the guard false rather than fail, so that a guard can be written for a backend before it exists.
8. As a Ruddy programmer, I want `@if` with no value, a non-struct value, or an empty struct refused in plain English as needing a condition, so that a forgotten condition is not silently true.
9. As a Ruddy programmer, I want an unknown field such as `@if {taget: "js"}` refused, naming the field and the fields that exist, so that a typo cannot silently drop a definition.
10. As a Ruddy programmer, I want a `target` that is not a string refused at the value, so that the shape of the condition is checked.
11. As a Ruddy programmer, I want a malformed guard to keep its definition, so that the one complaint is not followed by unresolved-name complaints for everything that used it.
12. As a Ruddy programmer, I want a surviving `@if` carried as ordinary metadata, so that tools can see which target a definition was written for.
13. As a debugger user, I want the document's configured target to decide its guards, so that the page shows what a build of that target would compile.
14. As a compiler maintainer, I want a dependency's cached artifact keyed by every fact of the build it was compiled for, so that two builds that differ in target or platform never read each other's cache entry.
15. As a Ruddy programmer, I want `platform = "web"` in `Ruddy.toml`, defaulting to `node`, so that a build can say where its output runs.
16. As a Ruddy programmer, I want `@if {platform: "web"}` to guard a definition by platform alone, and `@if {target: "js", platform: "web"}` by both, so that a guard names only the facts it cares about.
17. As a debugger user, I want a platform chip beside the target chip, saved with the document, so that the page shows what a build for that platform would compile.

## Semantics

- The guard is judged in the bundle loader, before any module file is read
  under the definition and before lowering mints any name. Excluded inline
  code is still lexed and parsed, so its syntax errors are still reported.
- A definition with two `@if`s, or a guard with a repeated field, is kept
  unjudged: the repeat is the duplicate lowering already refuses, and keeping
  the definition is what lets lowering see it, so no misspelling can drop a
  definition in silence.
- Several fields in the struct must all hold.
- Excluded definitions leave no trace in the tree, the IR, or the artifact.

## Diagnostics

All are bundle-stage errors, recoverable: the definition is kept.

| code | title |
| --- | --- |
| `condition-missing` | `@if` needs a condition |
| `condition-not-struct` | an `@if` condition is a struct |
| `condition-unknown-field` | this condition is not known (help names every fact) |
| `condition-not-string` | a condition's value is a string |

## Implementation Decisions

- `bundle::Environment` is an ordered map of fact name to value; the loader
  judges any field by looking its name up, so adding a fact touches the
  driver only. `bundle::load` takes one.
- The CLI's `Build { target, platform }` is derived from the root manifest
  and produces the environment; the debugger builds one from its request.
- `GraphCompiler` records the root's build and passes it to every project it
  compiles; the multi-root dependency-graph APIs, which have no single root,
  let each root use its own manifest.
- `cache::key` digests every fact alongside the sources and dependency keys.
- The debugger's in-process dependency memo keys on the build too.
- The backend's entry adapter and epilogue are Node's, so an executable with
  `platform = "web"` is refused where the manifest is read
  (`platform-unsupported`), and the debugger reports the same at its entry
  stage. A web library builds as any other: nothing a library emits touches
  Node. A web entry adapter is separate work.

## Out of Scope

- Negation, `else`, or any-of forms.
- Facts other than `target` and `platform`; `arch` waits for a backend that
  cares.
- Platform-specific code generation and entry checking.
- Guards on local definitions or expressions; attributes are refused there
  already.
