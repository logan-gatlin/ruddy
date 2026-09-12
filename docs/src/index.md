---
doc: true
---

# The Ruddy Book

Ruddy is an early-stage [functional language](dictionary.md#functional-language) with [structural types](dictionary.md#structural-typing), [type inference](dictionary.md#type-inference), and [algebraic effects](dictionary.md#effect).
This book introduces the language to intermediate programmers familiar with non-functional languages.
It assumes programming and undergraduate-level computer science knowledge, but no prior functional-programming experience.

The central questions are how expressions evaluate, what their types require, and how functions compose into programs with understandable behavior.
Game objects, movement, state machines, and replayable combat provide the examples.
Small examples establish rules; exercises apply those rules to new cases; a complete combat replay brings them together.
The emphasis is on game logic and tools that can be tested independently of an engine.

## Getting started

[Chapter 1](book/getting-started.md) establishes a project with live editor diagnostics and a first runnable program.
The [installation guide](download.md) covers the CLI and editors.
The [tooling appendix](book/tooling.md#trying-chapter-fragments) supplies a practice-project layout for trying the smaller examples.
Unless explicitly marked as complete files, snippets share the definitions introduced earlier in their chapter.

The main reading path runs through Chapter 12.
Chapters 13–15 develop advanced interfaces and integration when a project needs them.
The [standard-library reference](std/bundle.md), [syntax reference](grammar.md), and [dictionary](dictionary.md) support lookup throughout.

## Functions and values

Part I establishes evaluation and application.

1. [Getting started with Ruddy](book/getting-started.md)
2. [Values, expressions, and bindings](book/expressions.md)
3. [Functions and application](book/functions.md)

## Types and data

Part II develops representations, their type requirements, and abstractions over repeated computation.

4. [Modeling data and matching its shape](book/data.md)
5. [Understanding inferred and structural types](book/types.md)
6. [Higher-order programming](book/higher-order.md)

## Effects

Part III connects program organization with effectful operations and explicit state.

7. [Organizing programs](book/modules.md)
8. [Effects and handlers](book/effects.md)
9. [Mutable state and regions](book/state.md)

## Programs that interact with the world

Part IV crosses environmental boundaries and develops a complete application.

10. [Input, output, and failure](book/io.md)
11. [Structured data and network requests](book/external-data.md)
12. [Worked program: a combat replay](book/report.md)

## Advanced interfaces and integration

Part V examines relationships that require richer types and explicit host contracts.

13. [Rows, presence, and richer interfaces](book/rows.md)
14. [Hidden types, mirrors, and generic operations](book/reflection.md)
15. [JavaScript interoperability](book/interoperability.md)

## Game problems and type relationships

| Problem | Technique | Example |
| --- | --- | --- |
| Move objects with different component fields | Open and shared rows | [Geometry and movement](book/types.md#following-a-requirement) |
| Restrict operations to a known game state | Related field presences | [Pausing and resuming](book/rows.md#carrying-a-relationship-into-the-result) |
| Keep component combinations consistent | Inferred presence constraints | [Velocity and AI inputs](book/rows.md#presence-is-inferred-from-program-shape) |
| Restrict selections to equipped weapons | A row shared by a struct and a sum | [Loadouts](book/rows.md#rows-describe-entries-not-a-runtime-container) |
| Make time-dependent rules reproducible | Effects handled with fixed inputs | [Clocks and pure wrappers](book/effects.md#a-pure-wrapper-around-an-effectful-function) |
| Update actors with different private state | Hidden types with related operations | [Actor packages](book/reflection.md#different-actor-states-in-one-update-list) |
| Replay combat and retain checkpoints | Pure rules and immutable updates | [Combat replay](book/report.md) |

## Appendices and reference

- [A. Installation, editors, and CLI](book/tooling.md)
- [B. Syntax reference](grammar.md)
- [C. Dictionary](dictionary.md)
- [D. Numbers, strings, and runtime behavior](book/runtime.md)
- [E. Selected exercise answers](book/answers.md)
- [Generated standard-library reference](std/bundle.md)
- [Prelude and standard-library configuration](standard-library.md)
- [Detailed platform and representation contracts](platform-apis.md)
- [Hello World and FizzBuzz walkthrough](hello-world.md)

## About this draft

This first draft draws on the teaching approach of Cornell's [OCaml Programming: Correct + Efficient + Beautiful](https://cs3110.github.io/textbook/cover.html): explicit evaluation and typing explanations, focused examples, and exercises for programmers learning functional ideas.
Its prose and examples are written for Ruddy and its own language rules.
The generated standard-library pages remain the API reference; the chapters link to those pages rather than maintain duplicate signature catalogs.
