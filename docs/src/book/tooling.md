---
doc: true
---

# A. Installation, editors, and CLI

The [installation guide](../download.md) contains the supported source installation command and editor configuration.
The recommended daily workflow uses live language-server diagnostics in a configured editor.
The book's executable examples target JavaScript on Node.
The CLI also supports libraries and artifact-only builds.

## Trying chapter fragments

Unless a chapter identifies a complete file or program, its snippets are definitions in a shared chapter scope.
A later snippet may use definitions from an earlier snippet in that chapter.
Snippets from different chapters need separate scopes because examples can reuse names.

A practice project can place the chapter definitions in `src/examples.rud`.
The following complete `src/main.rud` loads that file as a bundle-private module and prints a string named `status`:

```ruddy filename="src/main.rud"
@private module examples
let main = fn _ => println examples::status
```

For an initial check, `src/examples.rud` can contain:

```ruddy filename="src/examples.rud"
let status = "Arena ready"
```

The language server checks the chapter definitions as they are edited and shows inferred types on hover.
After saving, `ruddy run` executes the entry point to inspect a result.
`ruddy check` remains useful for scripted verification and environments without an editor integration.
To inspect a chapter result of any type, pass it through `display`, for example `println (display examples::status)`.
[`display`](../std/str.md#display) formats structured values with indentation, quotes strings, and preserves numeric suffixes.
Use [`debug`](../std/str.md#debug) in the same way for single-line output.
Use `println examples::status` when the result is already text that should be printed directly.
An expected type-error example should be tried separately from the valid definitions.

The private module keeps exploratory definitions inside the Ruddy bundle.
Without it, public definitions also become candidates for JavaScript export, whose host-callable contracts impose additional requirements described in [interoperability](interoperability.md).
This arrangement lets a chapter explore an operation before supplying its handler at an executable boundary.

## Commands

Commands operate on the bundle containing the current directory.
`ruddy --help` and a command's `--help` describe available options.

| Command | Behavior |
| --- | --- |
| `ruddy new path` | Create a new project directory and Git repository. |
| `ruddy check` | Type-check the project and validate its entry and target contracts. |
| `ruddy build` | Write compiled output under `build/`. |
| `ruddy run` | Build and run a JavaScript executable. |
| `ruddy clean` | Remove the project's build output. |
| `ruddy fmt` | Apply canonical source formatting. |
| `ruddy fmt --check` | Report formatting differences without writing them. |
| `ruddy fmt --stdin` | Format a source file read from standard input. |
| `ruddy doc` | Generate public API documentation. |
| `ruddy lsp` | Run the language server over standard input and output. |

`ruddy run` currently takes no additional program arguments.
After a build, Node can run the generated `build/<bundle-name>.js` with arguments.
The [combat replay](report.md#run-the-example) demonstrates that path.

## Project configuration

A minimal executable manifest specifies its identity and root source:

```toml
name = "hello"
version = "0.1.0"
kind = "executable"
root = "src/main.rud"
target = "js"
platform = "node"

[dependencies]
```

The executable must have a public `main` callable with unit.
Its result must satisfy the entry contract, and its effects must be closed and supported by the selected runtime.
Unresolved operations must be handled before reaching that boundary.
Top-level initialization cannot leave effects unhandled; [the effects chapter](effects.md#why-top-level-initialization-restricts-effects) explains why unordered definitions require this restriction.
A library uses `kind = "library"` and needs no executable entry point.

## Dependencies

Omitting `std` from the dependencies selects the default standard library.
`std = false` disables both that dependency and its prelude.
An explicit local path can select a different library source:

```toml
[dependencies]
std = { path = "../ruddy" }
```

This override is useful when working against the standard library from a local Ruddy checkout.
Ordinary installed projects can keep the generated dependencies table unchanged.
The [standard-library guide](../standard-library.md#configuring-the-library) explains prelude selection.

A Git dependency names an HTTPS repository and can select one `branch`, `tag`, or `rev`.
The following explicit std selection follows the repository's `main` branch:

```toml
[dependencies]
std = { git = "https://github.com/logan-gatlin/ruddy.git", branch = "main" }
```

The resulting lockfile records a resolved commit even when the selector names a moving branch.
Git dependencies are cached under `$RUDDY_HOME/cache/git`, normally `~/.ruddy/cache/git`.
`Ruddy.lock` records resolved revisions so later builds can use the same selections.
Removing a lockfile changes that reproducibility boundary; it should be an intentional dependency update rather than a routine build step.

## Targets and platforms

A target determines output, while a platform identifies the intended runtime environment.

| Choice | Meaning |
| --- | --- |
| `target = "artifact"` | Write the portable compiler artifact without a backend build. |
| `target = "js"` | Write the artifact and JavaScript output. |
| `platform = "node"` | Select the Node platform and its runtime handlers. |
| `platform = "web"` | Select the web platform for supported library output. |

The default target is `js` for an executable and `artifact` for a library.
The default platform is `node`.
Web executables currently require an entry adapter that is not available; a web library does not have that executable entry requirement.
Platform-specific APIs and browser restrictions still apply.

`@if` selects definitions using facts of the root project's build.
A definition can require `target`, `platform`, or both; all supplied conditions must hold.
This selection affects name resolution, so alternative definitions can provide one name on different builds.
The [syntax reference](../grammar.md#conditional-definitions) gives the form.

## Generated documentation

`ruddy doc` writes public declarations and `@doc` prose to the configured documentation directory.
The default directory is `docs/`; a project can set `[documentation].target` relative to its manifest.
Private definitions and dependencies are excluded.
Generated pages should be regenerated from declarations rather than maintained as a second source of signatures.
The book links to the existing [standard-library reference](../std/bundle.md) at the point each API becomes relevant.

---

[Book contents](../index.md) · [Standard-library reference](../std/bundle.md)
