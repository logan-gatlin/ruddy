# Program entry points and platform effects

## Agreed contract

- Manifests require `kind = "executable" | "library"`. Only libraries may be dependencies. Artifact headers preserve bundle kind.
- `target = "artifact"` emits an artifact without invoking a backend. `target = "js"` emits both artifact and Node.js ESM. Omitted targets default to artifact for libraries and JS for executables. Replace the old `lib` target without migration diagnostics.
- Executables define root-module `main`, callable with unit and returning a type compatible with unit through ordinary inference, including Never and safely instantiated polymorphism. The invocation's effect row must be closed. Libraries assign no special meaning to main.
- Top-level initialization cannot leak effects in either kind; locally handled effects remain permitted.
- Artifact executables validate entry compatibility and closed effects, deferring runtime support checks. JS executables also require their effects to be supported by Node. Both check and build enforce this contract; check emits no build products.
- Generated executable JS initializes globals then invokes main once under runtime handlers. Library JS only initializes and exports. Run requires an executable targeting JS and launches its generated file; existing runner configuration selects the launching command only.
- Std declares root Console with write and write_error operations, each String -> (), writing without newlines to stdout/stderr; and Process with exit : Nat -> Never.
- Exit saturates its Nat argument to 255 before passing it to Node, does not resume, and permits pending console output to drain. Normal return completes with exit status zero.
- Runtime handlers are ordinary outermost handlers. Local handlers can intercept Console and Process; a Process handler can raise an alternative answer instead of resuming.
- Runtime recognition uses the complete structural effect identity, never just the name or declaration origin. Std overrides and std = false remain allowed. Pure executables require no std.
- Std adds console::write, write_error, print, print_error and process::exit. Print helpers append a newline.
- Scaffold a JS executable with a unit-returning main, and update repository manifests and debugger support.
- No target triples, runtime manifest field, browser implementation, new saved-artifact-input CLI, filesystem/network/async effects, or migration layer.

## Verification and review

Confirmed public test seams: compilation/artifact APIs for entry validation and persistence; CLI temporary projects and Node execution for manifests, dependency restrictions, emitted outputs, handlers and exit behavior. Rust tests run only through `just test`.

Review baseline: c5b7e0ab70f63a97050e296cdca3b0887bbf4062. Review against this contract and repository standards before the final commit.
