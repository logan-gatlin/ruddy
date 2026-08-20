# tree-sitter-ruddy

The tree-sitter grammar for ruddy — the `.hc` files `src/main.rs` compiles and
the debugger edits. It exists so that editors can colour, fold and navigate a
source file without running the compiler.

`src/token.rs` and `src/parse.rs` are the source of truth. Every rule in
`grammar.js` is named after the production it mirrors there, and the comments
say which one. **A change to either belongs here in the same commit** — see the
repository's `CLAUDE.md`.

## Layout

| Path | What it is |
| --- | --- |
| `grammar.js` | The grammar. The file to edit. |
| `tree-sitter.json` | Grammar metadata: the `hc` extension, and where the queries live. |
| `queries/highlights.scm` | What each node is painted as. |
| `queries/locals.scm` | Scopes, what binds in them, and what a name may resolve to. |
| `queries/folds.scm` | The forms worth folding away. |
| `test/corpus/*.txt` | The corpus tests, one file per part of the grammar. |
| `src/` | Generated: `parser.c` and friends. Checked in so an editor can build the parser without the CLI. Never edited by hand. |

## Working on it

```sh
npm install                 # the tree-sitter CLI, once
just grammar                # regenerate the parser and run the corpus tests
```

Or, by hand, from this directory:

```sh
npx tree-sitter generate            # rebuild src/ from grammar.js
npx tree-sitter test                # run test/corpus
npx tree-sitter test -u             # rewrite the expected trees — read the diff
npx tree-sitter parse ../demo.hc    # the whole demo, as a tree
npx tree-sitter highlight ../demo.hc
npx tree-sitter query queries/highlights.scm ../demo.hc
```

`demo.hc` ends with three deliberately broken definitions, so a parse of it is
expected to report errors on those lines and nowhere else.

Adding a corpus test: write the source and leave `(source_file)` as the expected
tree, then run `npx tree-sitter test -u` and check that the tree it filled in is
the one the parser in `src/parse.rs` would have built. The point of the test is
that judgement, so do not skip it.

## What the grammar says, that the token stream cannot

The debugger paints from the compiler's own tokens, which is why a lexer gap
shows up as red text there. A tree knows more than a token does, and the
highlight query spends the difference: a name in a type position is a type, a
name before a colon in a struct is a field, a name in a `fn` header is a
parameter, and a definition whose body is a `fn` is a function. Tags, effects
and variables keep the classes `debug/src/stage/tokens.rs` gives them.

## Where it differs from the compiler

The grammar is faithful about what parses — the reserved words, the contextual
ones, the associativities, and the two-token lookaheads all match `parse.rs` —
with these exceptions, none of which a correct program can tell apart:

- **Malformed literals parse.** `12abc` and a number too large for `u64` are
  one `natural` node each. The lexer reads them as one lexeme too, and then
  refuses them; a grammar has nowhere to say "this literal is broken".
- **What lowering refuses is not the grammar's business.** A hole in a
  declaration, a `..` in a declared type, an `effect` mixing operations with
  aliases: `parse.rs` builds a tree for each of these and `ir.rs` complains.
  So does this.
- **Names are `\p{Alphabetic}`**, which is what `char::is_alphabetic` means,
  and name characters are that plus `\p{N}` and `_`, which is what
  `char::is_alphanumeric` means. Exact today; worth re-checking if the lexer's
  test ever changes.

## Using it from an editor

The parser is generated with ABI 15, which is what the reserved words need: a
parser generated with `--abi 14` still builds, and silently reads `let match =
1` as a definition called `match`. So the runtime loading it has to be
tree-sitter 0.25 or newer, and `generate` must be left at its default ABI.

### Helix

```sh
just helix
```

That compiles `src/parser.c` into `~/.config/helix/runtime/grammars/ruddy.so`,
copies the queries to `~/.config/helix/runtime/queries/ruddy/`, and appends the
language entry to `~/.config/helix/languages.toml` — unless one naming `ruddy`
is already there, in which case it leaves the file alone. Re-running it after
`just grammar` reinstalls the parser and the queries.

`hx --health ruddy` should report a tree-sitter parser and highlight queries.
There are no textobject or indent queries, so those two stay unticked.

The `[[grammar]]` block it appends points at this directory, so `hx --grammar
build` can rebuild the parser from the same place; move the repository and that
path is what needs fixing.

### Neovim

With `nvim-treesitter` installed:

```lua
vim.filetype.add({ extension = { hc = "ruddy" } })

require("nvim-treesitter.parsers").get_parser_configs().ruddy = {
  install_info = {
    url = "/path/to/ruddy/treesitter",
    files = { "src/parser.c" },
  },
  filetype = "ruddy",
}
```

Then `:TSInstall ruddy`, and copy `queries/` to
`~/.config/nvim/queries/ruddy/`.
