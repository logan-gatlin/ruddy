# Ruddy

Ruddy projects are configured by `Ruddy.toml`. Dependencies can be local paths or HTTPS Git repositories:

```toml
[dependencies]
local = "../local"
http_core = { path = "../http-core", bundle = "http-core" }
remote = { git = "https://example.com/team/remote.git", branch = "main" }
release = { git = "https://example.com/team/release.git", tag = "v1.0.0" }
pinned = { git = "https://example.com/team/pinned.git", rev = "0123456789abcdef0123456789abcdef01234567" }
aliased = { git = "https://example.com/team/http-core.git", bundle = "http-core" }
```

A Git dependency accepts at most one of `branch`, `tag`, or `rev`; without one, Ruddy resolves the remote default branch. Only HTTPS URLs are accepted. A successful resolution records the full commit in the root project's `Ruddy.lock`, including transitive Git dependencies. Commit `Ruddy.lock` for reproducible builds. A failed resolution or compilation does not replace it.

Git repositories are fetched and checked out with pure-Rust `gix` and Rustls—Ruddy never invokes a Git executable. Checkouts live under `$RUDDY_HOME/git/checkouts`; if `RUDDY_HOME` is unset the default is `$XDG_CACHE_HOME/ruddy`, or `$HOME/.cache/ruddy`. Resolution can populate this cache even if compilation later fails. `compile` and `compile_graph` write no artifacts, but may fetch dependencies and update `Ruddy.lock`; `build` writes artifacts only after the entire graph compiles.

`ruddy new NAME` creates a project and initializes its repository. `Ruddy.lock` is generated on the first dependency resolution and is intentionally not ignored; only `/build/` is listed in a new project's `.gitignore`.
