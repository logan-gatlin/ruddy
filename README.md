# Ruddy

Ruddy is a structurally typed functional language and compiler with a JavaScript backend and a browser-based phase debugger.

## Installation

A source checkout and a Rust toolchain are currently required. [`just`](https://just.systems/) provides the installation task:

```sh
git clone https://github.com/logan-gatlin/ruddy.git
cd ruddy
just install
```

This installs the `ruddy` executable into Cargo's binary directory and installs the matching standard-library source project at `$RUDDY_HOME/std`. A non-empty `RUDDY_HOME` selects the Ruddy data directory; otherwise it is `$HOME/.ruddy`. The first std install uses a portable same-filesystem directory rename after staging. Re-running `just install` destructively replaces the previous std tree with an atomic directory-entry exchange, so the `std` path is never missing or a partially copied tree; this namespace guarantee is not a transactional snapshot for readers that open multiple files across the exchange. An interrupted copy leaves the old tree in place. Atomic replacement requires Linux, GNU `mv` with `--exchange`, and filesystem support for `renameat2(RENAME_EXCHANGE)`; the installer checks these capabilities before copying and reports how to remove the old tree for a portable first install when they are unavailable. There is no portable atomic directory-exchange operation, so replacement does not silently fall back to a remove-and-rename window. `cargo install --locked --path ./cli` alone installs only the binary. Ensure Cargo's binary directory (usually `$HOME/.cargo/bin`) is on `PATH`.

## Projects and dependencies

Ruddy projects are configured by `Ruddy.toml`. Dependencies can be local paths or HTTPS Git repositories:

```toml
[dependencies]
local = "../local"
http_core = { path = "../http-core", bundle = "http-core" }
remote = { git = "https://example.com/team/remote.git", branch = "main" }
release = { git = "https://example.com/team/release.git", tag = "v1.0.0" }
pinned = { git = "https://example.com/team/pinned.git", rev = "0123456" }
aliased = { git = "https://example.com/team/http-core.git", bundle = "http-core" }
```

A Git dependency accepts at most one of `branch`, `tag`, or `rev`; without one, Ruddy resolves the remote default branch. `rev` currently accepts an unambiguous 7–40 digit hexadecimal commit prefix reachable through the repository's normally fetched branches or tags; arbitrary revision expressions and other advertised ref names are not yet accepted. Only HTTPS URLs are accepted, including after ambient Git URL rewriting. A successful resolution records the full lowercase commit in the root project's `Ruddy.lock`, including transitive Git dependencies. Commit `Ruddy.lock` for reproducible builds. A failed resolution or compilation does not replace it.

Git repositories are fetched and checked out with pure-Rust `gix` and Rustls—Ruddy never invokes a Git executable. `RUDDY_HOME` uses a non-empty explicit override when set and otherwise defaults to `$HOME/.ruddy` (independent of `XDG_CACHE_HOME`). All global Git cache data lives beneath `$RUDDY_HOME/cache/git`: checkouts are in `$RUDDY_HOME/cache/git/checkouts`, with temporary clones and the advisory lock alongside them under that cache directory. The project-local `Ruddy.lock` is not part of this cache. Resolution can populate the cache even if compilation later fails. Cached trees are restored to their locked commit under a cross-process lock before every compilation. `compile` and `compile_graph` write no artifacts, but may fetch dependencies and update `Ruddy.lock`; `build` writes local artifacts only after the entire graph compiles and never writes into Git cache checkouts.

`ruddy new NAME` (or `ruddy n NAME`) creates a project and initializes its repository. `ruddy build` (`ruddy b`) compiles and writes artifacts, `ruddy run` builds and executes a JavaScript target, `ruddy check` compiles without writing artifacts, and `ruddy clean` removes project build output. `Ruddy.lock` is generated on the first dependency resolution and is intentionally not ignored; only `/build/` is listed in a new project's `.gitignore`.

### Standard library

Every project implicitly receives a dependency named `std`, resolved from `$RUDDY_HOME/std`. The dependency is injected before declared dependencies and is available through qualified `std::…` names. The bundled `std@0.1.0` is initially empty; its source project lives in [`std/`](std/).

A project can disable this dependency or replace it with any normal path or Git dependency using the top-level `std` field:

```toml
# Bootstrap a project without the standard library:
std = false

# Or select another std project (instead of `false`):
# std = "../my-std"
# std = { path = "../foundation", bundle = "foundation" }
# std = { git = "https://example.com/my-std.git", tag = "v1.0.0" }
# std = { git = "https://example.com/foundation.git", rev = "0123456", bundle = "foundation" }

[dependencies]
```

The optional `bundle` is the dependency project's actual manifest name when that differs from the source alias: the second example is still referenced as `std::…`, but its artifact identity is `foundation`. Relative paths are resolved from the manifest that declares them. Git overrides use the normal shared cache and selectors, and every transitive resolution is recorded in the root project's lockfile rather than a dependency-local lockfile.

`std = true` is invalid: omit the field to use the installed library. The source alias `std` is reserved and cannot also appear in `[dependencies]`, though another alias may legally target a bundle actually named `std`. `ruddy new` deliberately omits the setting and uses the installed default.

Each manifest—including transitive path and Git dependencies—resolves its own `std` setting. A custom or bootstrap library should generally declare `std = false` to avoid depending on the installed library itself. Canonical projects are still deduplicated, multiple std bundle identities or versions may coexist, and the usual error applies if the same name and version come from different locations. If the default installation is missing, unreadable, malformed, or names the wrong bundle, Ruddy reports how to run `just install`, configure an override, or set `std = false`.

The debugger persists the same default, disabled, or custom setting. Its title-bar **std** toggle disables the effective setting without forgetting a custom specification, then restores that specification when re-enabled; std is shown first in **Dependencies** and flows through Artifact, Linked Artifact, and JavaScript. The debugger may read the exact canonical installed `$RUDDY_HOME/std` tree outside its scratch folder. Custom local overrides remain confined to the scratch sandbox, while Git std overrides use the existing trusted checkout boundary.

## JavaScript target

A project is an artifact-only library by default. Set the root manifest's target to JavaScript to also emit an ECMAScript module:

```toml
name = "app"
version = "0.1.0"
root = "main.hc"
target = "js"

[dependencies]
```

`ruddy build` then writes both `build/app.artifact` and `build/app.js`. Generation uses the fully linked in-memory root artifact, so the JavaScript module is self-contained with respect to Ruddy dependencies. A dependency's own `target` never causes JavaScript output while building a parent, and Ruddy never writes into an immutable Git checkout. The default `target = "lib"` writes only the canonical artifact. `ruddy check` generates neither file.

The generated file is deterministic, portable JavaScript ESM. Root values are named exports, while nested Ruddy modules become frozen, prototype-free namespace objects:

```js
import { main, util } from "./build/app.js";
main;
util.map;
```

Ruddy `extern` declarations are resolved by walking their dotted target from `globalThis` during module initialization; the embedding environment must provide those values. Function externs are bound to the object owning the final path segment, so host methods retain their `this` receiver.

Ruddy `Nat`, `Int`, and `Real` values use JavaScript `Number`; integers beyond 2^53 can therefore lose precision. Natural subtraction saturates at zero, integer division truncates toward zero, and real division uses ordinary JavaScript division.

Compiler integrations can invoke the backend directly with `ruddy_js::generate(&artifact)`. The argument must be the final linked [`ruddy::artifact::Artifact`]; generation returns the module source or a validation error and performs no filesystem I/O. The debugger always shows this same output in its **JavaScript** phase, regardless of the manifest target.

Switching a project back to `"lib"` leaves an existing JavaScript file in place until `ruddy clean` removes the build directory.

### Running JavaScript

`ruddy run` accepts no path or program arguments and is available only when the root manifest sets `target = "js"`. It first performs the same complete, dependency-first build and atomic installation as `ruddy build`, then loads and evaluates the newly installed `build/<name>.js` as an ECMAScript module. Loading the module is the entire entry point: Ruddy does not require, inspect, call, or print a `main` export. A library target is rejected and stale JavaScript is never executed. Since installation finishes before execution starts, completed build files remain available when module initialization later fails.

Boa is the default runtime. A root project can instead configure a shell command in `Ruddy.toml`:

```toml
[run]
js = "node"
```

Ruddy runs this command from the project directory with the generated JavaScript file path appended as its final argument. The command may include shell syntax and arguments, for example `js = "node --enable-source-maps"`. A failure to start the shell or a nonzero runner exit status fails `ruddy run`; installed build files remain available. Run settings in dependency manifests have no effect.

The embedded Boa runtime provides `console`, timers, `queueMicrotask`, text encoding, `URL`, base64, `structuredClone`, abort APIs, `process.cwd()` and `process.env`, and blocking network-enabled `fetch`. It intentionally provides no filesystem or standard-input API and Ruddy adds no custom host API. Boa's queued promises, microtasks, and timers are driven through completion; code that continually schedules more work can therefore keep `ruddy run` alive. Parse, linking, evaluation, rejected-promise, and queued-job errors exit with status 1 and include Boa's available JavaScript stack frames. A successful run is silent except for output produced by the module or runtime.
