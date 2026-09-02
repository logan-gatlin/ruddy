# Repository instructions

## Tests

- Run the Rust test suite only through `just test`.
- Never invoke `cargo test` directly, including for focused tests or with additional test arguments.
- The `just test` recipe places the complete test process tree in a memory-limited systemd scope. Bypassing it can exhaust system memory and terminate the coding-agent session.
