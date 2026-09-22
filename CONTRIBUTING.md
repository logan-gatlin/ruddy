For every new feature added to the compiler, make sure the debugger (./debug/)
supports the change as well. Consider adding new features to the debugger
if they would be helpful. Every compiler phase should get a new tab.
For every grammar change, update the treesitter grammar (./treesitter/) and its
highlights; `just grammar` regenerates the parser and runs its corpus tests.

Every test in the workspace lives in the `ruddy-tests` crate (./tests/), one
module per module under test. `src/` and `debug/src/` carry no `#[cfg(test)]`
modules of their own, so tests reach the code through the same public API any
other consumer would; a test that needs an item the crate does not export means
the item should be exported.

Validate changes with focused regression tests. Run Rust tests through
`just test` so test executables and their children stay within the memory limit.
For example, `just test -p ruddy-tests --lib inference::` selects the inference
tests without building unrelated test harnesses. With no package selection,
`just test` includes the whole workspace and its doctests. Tests use a persistent
cache in `target/test-ruddy-home` unless `RUDDY_HOME` is explicitly set.

The type system is designed around three constraints
1. Inference/checking must provably terminate
2. Types are structural, not nominal
3. Inference should be total (with maybe very few exceptions)
