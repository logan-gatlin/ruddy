---
doc: true
bookNavigation:
  previous:
    path: "/index.html"
    title: "The Ruddy Book"
  next:
    path: "/book/expressions.html"
    title: "2. Values, expressions, and bindings"
---

# 1. Getting started with Ruddy

Ruddy is an early-stage [functional language](../dictionary.md#functional-language) with structural types, type inference, and algebraic effects.
This book assumes experience building programs in a non-functional language.
Functions, data structures, debugging, and command-line tools are familiar territory; Ruddy's way of connecting them is the subject of the book.

A functional program often describes a result by composing expressions that transform data.
That changes the questions asked during implementation: what value does this expression produce, what does this function require, and which operations must the surrounding program provide?
The chapters develop answers through game objects, movement, state machines, and replayable simulation.
The examples assume no particular game engine; the first program runs in a terminal, and later chapters keep gameplay rules separate from input and rendering.

## A first program

The [installation guide](../download.md) covers the CLI and editor setup.
A supported editor runs Ruddy's [language server](../download.md#set-up-an-editor) to show diagnostics as source changes, including unsaved edits.
That immediate feedback is the normal way to develop the examples.
The following path also requires both `ruddy` and Node.js on the terminal's search path.
A new project is created in a directory that does not already exist:

```sh
ruddy new arena
cd arena
```

Opening the new project folder in the configured editor starts language-server analysis.
`Ruddy.toml` identifies the project to the editor; opening the folder gives the server that context.
The generated `src/main.rud` contains an entry point that returns without printing.
Replacing its contents with the following program produces an observable result:

```ruddy filename="src/main.rud"
let display_name = fn entity => entity.name
let player = { name: "Scout", health: 100n }
let main = fn _ => println (display_name player)
```

The editor reports any source errors while the program is being written.
Once the diagnostics are resolved and the file is saved, the following terminal command builds and executes the program from inside `arena`:

```sh
ruddy run
```

The output is:

```text
Scout
```

`let` associates a name with a value, and `fn` constructs a function.
`display_name player` applies a function to its argument; parentheses make that result the argument to `println`.
The [struct](../dictionary.md#struct) bound to `player` contains two fields, but `display_name` only uses `name`.
The type checker determines that requirement from the function body.

The runtime calls `main` with `()`, the [unit](../dictionary.md#unit) value.
The parameter `_` ignores that argument.
The [prelude](../std/prelude.md) supplies `println`, whose output operation is handled by the runtime.
[Effects and handlers](effects.md) later explain how that behavior can be supplied by a program instead.

## Feedback in the editor

`Ruddy.toml` identifies the project's root source and whether it is an executable or library.
The initial project already has the configuration needed for this example.
Diagnostics explain incompatible uses as code changes, hover reveals inferred types, and go-to-definition follows library operations to their source.
This keeps feedback beside the expression being edited rather than requiring a separate check after every change.
Running the program then answers a different question: whether its behavior matches the intended result.

The CLI remains useful for execution, builds, and automated checks:

| Command | Purpose |
| --- | --- |
| `ruddy check` | Check the program without producing build artifacts. |
| `ruddy build` | Compile the program without executing it. |
| `ruddy run` | Build and run the executable. |
| `ruddy fmt` | Format the project's source. |

The [tooling appendix](tooling.md) covers project configuration and the commands used beyond the first program.

## A useful failure

Changing the `name` field to `42n` makes the program ill-typed: the function returns a natural number, but `println` requires a string.
The editor immediately reports the conflicting requirements at the call.
Hovering over the related definitions helps follow the value from the numeric field to the string-consuming function.
The useful question is which use establishes each requirement; the underlined expression is not necessarily where the intended repair belongs.
Restoring the string makes the program valid again.

## Summary and exercises

A program is checked before it runs, calls use whitespace, and functions can state requirements on a value through the fields they access.
Editor diagnostics and inferred types supply evidence about those requirements before execution.

1. Explain why changing the health leaves the output unchanged.
2. Change the function body to `display entity.health` and predict the output before running it. The prelude's [`display`](../std/str.md#display) converts any value to readable text, preserving numeric suffixes.
3. Remove the field used by the function and identify which expression requires it.

[Selected answers](answers.md#getting-started) explain these observations.

