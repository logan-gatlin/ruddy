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

# Regenerate the tree-sitter parser and run its corpus tests.
grammar *args:
    #!/usr/bin/env bash
    # Needs npm; the CLI installs into `treesitter/node_modules`.
    set -euo pipefail
    cd "{{justfile_directory()}}/treesitter"
    [ -x node_modules/.bin/tree-sitter ] || npm install --silent --no-audit --no-fund
    node_modules/.bin/tree-sitter generate
    node_modules/.bin/tree-sitter test {{args}}

# Install the grammar and its highlights into the local Helix configuration.
helix:
    #!/usr/bin/env bash
    # The parser as the shared object Helix loads a grammar by, the queries
    # beside it, and the language entry appended to `languages.toml` when it is
    # not already there. The parser installed is the generated one in the tree,
    # so run `just grammar` first if `treesitter/grammar.js` has moved on.
    set -euo pipefail
    grammar="{{justfile_directory()}}/treesitter"
    config="${XDG_CONFIG_HOME:-$HOME/.config}/helix"
    runtime="$config/runtime"
    languages="$config/languages.toml"

    if [ ! -f "$grammar/src/parser.c" ]; then
        echo "no generated parser; run 'just grammar' first" >&2
        exit 1
    fi

    mkdir -p "$runtime/grammars" "$runtime/queries/ruddy"

    # What `hx --grammar build` does, minus the fetch: one translation unit,
    # no external scanner, under the name the language entry asks for.
    cc -O2 -fPIC -shared -I "$grammar/src" \
        -o "$runtime/grammars/ruddy.so" "$grammar/src/parser.c"
    echo "built $runtime/grammars/ruddy.so"

    cp "$grammar"/queries/*.scm "$runtime/queries/ruddy/"
    echo "copied $(ls "$grammar"/queries/*.scm | wc -l) queries to $runtime/queries/ruddy"

    # Appended rather than written: `languages.toml` is the editor's own file
    # and everything else in it is somebody else's language.
    if grep -qs 'name = "ruddy"' "$languages"; then
        echo "$languages already names ruddy — left as it is"
    else
        # Indented to the recipe's own margin, which `just` strips before bash
        # ever sees it, so what lands in the file starts at the left.
        cat >> "$languages" <<EOF

    [[language]]
    name = "ruddy"
    scope = "source.ruddy"
    injection-regex = "ruddy"
    file-types = ["hc"]
    indent = { tab-width = 2, unit = "  " }
    grammar = "ruddy"

    [[grammar]]
    name = "ruddy"
    source = { path = "$grammar" }
    EOF
        echo "appended the ruddy language to $languages"
    fi

    echo "open a .hc file, or check with: hx --health ruddy"

# Everything CI would run.
check: fmt-check clippy test

test:
    cargo test --workspace

build:
    cargo build --workspace

# Line and branch coverage for the compiler library. Branch coverage is a
# nightly-only rustc feature, hence `+nightly`.
cov *args:
    cargo +nightly llvm-cov --branch --workspace --ignore-filename-regex '/(tests|debug)/src/|/rustlib/' {{args}}

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
