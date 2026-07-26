For every new feature added to the compiler, make sure the debugger (./debug/)
supports the change as well. Consider adding new features to the debugger if
they would be helpful. Every compiler phase should get a new tab, at least. If
the debugger doesn't at least compile, the feature is not done.

Every test in the workspace lives in the `ruddy-tests` crate (./tests/), one
module per module under test. `src/` and `debug/src/` carry no `#[cfg(test)]`
modules of their own, so tests reach the code through the same public API any
other consumer would; a test that needs an item the crate does not export means
the item should be exported.
