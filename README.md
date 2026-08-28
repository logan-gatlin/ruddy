# Ruddy

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

The embedded Boa runtime provides `console`, timers, `queueMicrotask`, text encoding, `URL`, base64, `structuredClone`, abort APIs, `process.cwd()` and `process.env`, and blocking network-enabled `fetch`. It intentionally provides no filesystem or standard-input API and Ruddy adds no custom host API. Boa's queued promises, microtasks, and timers are driven through completion; code that continually schedules more work can therefore keep `ruddy run` alive. Parse, linking, evaluation, rejected-promise, and queued-job errors exit with status 1 and include Boa's available JavaScript stack frames. A successful run is silent except for output produced by the module or runtime.
