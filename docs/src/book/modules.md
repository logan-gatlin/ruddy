---
doc: true
bookNavigation:
  previous:
    path: "/book/higher-order.html"
    title: "6. Higher-order programming"
  next:
    path: "/book/effects.html"
    title: "8. Effects and handlers"
---

# 7. Organizing programs

A [module](../dictionary.md#module) groups related definitions under a name.
A useful module gives its callers a small set of operations that express a coherent purpose.
Its implementation can then change without requiring callers to repeat its decisions.
Top-level definitions need not follow dependency order, including when functions refer to each other.
The [initialization effect restriction](effects.md#why-top-level-initialization-restricts-effects) preserves that freedom by preventing declaration order from becoming the order of observable operations.

## Names and interfaces

An inline module can keep spawn-message formatting decisions together:

```ruddy
module Spawn =
  @private
  let prefix = "Spawn: "

  @doc "A debug message for an enemy spawn."
  let message = fn name => std::str::concat prefix name
end

let title = Spawn::message "Slime"
```

The qualified path uses `::`.
Unmarked definitions are public, while `@private` limits a definition to its declaring bundle.
This is a bundle boundary: it does not hide `prefix` from other modules in the same bundle.
A private module also hides the definitions beneath it from dependent bundles.

A `using` statement introduces shorter names without re-exporting the imported declarations.
The following import refers to the module above:

```ruddy
using Spawn::message as spawn_message
let short_title = spawn_message "Golem"
```

The [syntax reference](../grammar.md#modules-attributes-and-foreign-values) covers grouped imports, dependency aliases, and paths rooted at `bundle`, `self`, or `super`.

## Moving a module into a file

In a root file at `src/main.rud`, the declaration `module spawn` loads a module from its source file, such as `src/spawn.rud`.
That file contains the module's definitions without an enclosing inline module wrapper.
Calls then use `spawn::message`.
The [worked program](report.md) demonstrates this arrangement with complete files.

A [bundle](../dictionary.md#bundle) is the unit described by `Ruddy.toml`.
An executable has a public root `main`; a library provides definitions to dependent bundles.
Only libraries can be dependencies.
A module separates names inside a bundle, while a dependency connects bundles.

## Dependencies and the prelude

The default standard library is a dependency named `std`.
Its [prelude](../std/prelude.md) supplies selected names automatically, while `std::array` and other modules require qualification or imports.
Explicit imports and local definitions can shadow prelude names.

A project can depend on another local library through a path:

```toml
[dependencies]
game_rules = { path = "../game-rules" }
```

`game_rules` is the alias used in source paths.
[Tooling](tooling.md#dependencies) describes default std selection, Git dependencies, and lockfiles.
A dependency's location and version should not be embedded in the language-level module interface.

## Contracts and verification

`@doc` attaches prose to a public declaration.
A useful contract explains the result, edge cases, and any input requirement that the type does not express.
`ruddy doc` generates a project's API pages from its public declarations and documentation.
The [generated std reference](../std/bundle.md) is an example of that output, and remains the source for its complete signatures.

A small verification executable can call a library operation on representative inputs and report whether the results match expectations.
Type checking establishes compatibility of uses; it does not establish a gameplay rule such as the right damage falloff or the correct order of turn resolution.
Tests for such rules need observable expected results.
A library's public interface is a useful boundary for these checks.

## Summary and exercises

Modules organize names, bundles establish dependency boundaries, and public contracts describe the behavior callers can rely on.

1. Move `Spawn` into a file module and update its call sites.
2. Explain why `@private` does not prevent another module in the same bundle from using a definition.
3. Write three input/output checks for `Spawn::message`, including an empty name.

[Selected answers](answers.md#modules) describe the file layout.

