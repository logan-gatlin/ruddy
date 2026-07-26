# ruddy — compiler and debugger tasks.
#
# `just dev` is the one to know: it serves the debugger and rebuilds it whenever
# the compiler changes, so editing `src/*.rs` refreshes the open page by itself.

port := "7878"
dev_dir := justfile_directory() / "debug/.dev"

_default:
    @just --list --unsorted

# Serve the debugger, rebuilding and restarting on any compiler change.
dev:
    #!/usr/bin/env bash
    set -uo pipefail
    mkdir -p "{{dev_dir}}"
    log="{{dev_dir}}/build-error.log"
    good="{{dev_dir}}/ruddy-debug"
    build=0

    while true; do
        build=$((build + 1))
        printf '\n\033[2m── build %s ──────────────────────────────────────\033[0m\n' "$build"

        if cargo build -p ruddy-debug 2>&1 | tee "$log"; then
            cp "{{justfile_directory()}}/target/debug/ruddy-debug" "$good"
            : > "$log"
            error=""
        elif [ -x "$good" ]; then
            # A broken build must not be a dead tab: keep serving the last good
            # binary and hand it the rustc output to show in the error strip.
            printf '\033[31mbuild failed — serving the last good binary\033[0m\n'
            error="$log"
        else
            printf '\033[31mbuild failed and there is no previous binary; waiting…\033[0m\n'
            sleep 2
            continue
        fi

        RUDDY_DEBUG_SUPERVISED=1 \
        RUDDY_DEBUG_BUILD="$build" \
        RUDDY_DEBUG_BUILD_ERROR="$error" \
        RUST_BACKTRACE=1 \
            "$good" --port {{port}}

        # 75 is the server saying a source file changed; anything else is the
        # server exiting on its own terms, and so is this loop.
        code=$?
        [ $code -eq 75 ] || exit $code
    done

# Serve the debugger once, without the rebuild supervision.
debug *args:
    cargo run -p ruddy-debug -- --port {{port}} {{args}}

# Compile demo.hc with the CLI compiler.
demo:
    cargo run --quiet

# Everything CI would run.
check: fmt-check clippy test

test:
    cargo test --workspace

build:
    cargo build --workspace

clippy:
    cargo clippy --workspace --all-targets

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

# Drop the supervisor's scratch state (last good binary, build log).
clean-dev:
    rm -rf "{{dev_dir}}"

clean:
    cargo clean
    rm -rf "{{dev_dir}}"
