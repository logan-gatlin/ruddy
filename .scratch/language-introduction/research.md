# An approachable introduction to Ruddy

Researched 2026-09-11 against the working tree based on `79e6ffa` and the primary sources below. This is an editorial recommendation, not an accepted spec or a replacement for the published introduction. The checkout contains existing changes; observations concern the files as read. Official tutorials show deliberate documentation choices, but do not establish comparative learning effectiveness. No reader study was performed.

## Recommendation

Gear the introduction to intermediate programmers familiar with non-functional languages. This audience is a user requirement, clarified after the initial research. Assume competence with functions, control flow, data structures, modules, debugging, and command-line tools; do not assume functional-programming vocabulary or a particular source language. Explain Ruddy's syntax and semantics at the points where those readers need to adjust their existing mental models.

Use the user-selected Cornell OCaml textbook as the principal pedagogical model: a concept-led introduction that builds a way to reason about programs. Keep setup short, then explain expressions, evaluation, functions, types, data, abstraction, and effects through focused examples and exercises. A contact example can illustrate a concept without becoming the organizing project for the whole guide. This recommendation supersedes the earlier emphasis on one continuous task; see the Cornell analysis below.

The surveyed guides support declaring prior experience explicitly: Go lists programming experience among its prerequisites, Rust distinguishes learning Rust from learning programming, and Gleam declares prior programming experience. Ruddy's specific intermediate audience comes from the user's direction, not from these sources. [Go prerequisites](https://go.dev/doc/tutorial/getting-started), [Rust audience](https://doc.rust-lang.org/book/ch00-00-introduction.html#who-this-book-is-for), [Gleam tour](https://tour.gleam.run/)

Give the documents distinct jobs. The landing page provides orientation: why try Ruddy, who the guide serves, a small sample, and a next action. The tutorial supplies a guided task; the reference supports precise lookup afterward. This proposed division applies [Diátaxis's distinction between tutorials and reference](https://diataxis.fr/start-here/) to the existing site. It does not require a large new documentation hierarchy.

## Pointers from the Cornell OCaml textbook

The user identified [OCaml Programming: Correct + Efficient + Beautiful](https://cs3110.github.io/textbook/cover.html) as an excellent model. Reading covered the preface, introductory motivation, expressions and functions sections, with additional sampling of data, higher-order programming, and exercises. This is a targeted reading of its teaching approach, not a claim to have read every chapter or watched the videos.

### Audience and ambition

The book assumes imperative programming experience and no prior functional programming. Its Cornell audience has typically studied Python and Java; it also assumes discrete mathematics. That closely matches Ruddy's intended programming audience, but the mathematics prerequisite need not transfer automatically. [About This Book](https://cs3110.github.io/textbook/chapters/preface/about.html)

Its introduction motivates learning through changes in programming practice: immutability, abstraction, types, and understanding language behavior. My inference for Ruddy: lead with the perspective readers will acquire, then explain how the language supports it. Use concrete benefits and avoid promises that types or immutability eliminate all testing or debugging. [Better Programming Through OCaml](https://cs3110.github.io/textbook/chapters/intro/intro.html)

### Explain rules readers can apply independently

The expressions section separates syntax, evaluation rules, and typing rules. It traces a binding's evaluation and uses scope examples to distinguish shadowing from assignment. It connects unfamiliar expression forms to familiar imperative constructs. [Expressions](https://cs3110.github.io/textbook/chapters/basics/expressions.html)

For Ruddy, use a recurring section pattern: motivating example, written form, evaluation, type constraints, a revealing mistake, and exercises. Introduce the terms static and dynamic semantics with plain-language explanations. This is a writing recommendation, not a requirement to present a formal calculus. A reader should be able to predict an unfamiliar example's behavior after learning the rule.

The functions section derives inferred types from constraints in the body and develops partial application through equivalent function forms. It makes the association of calls and function types explicit. [Functions](https://cs3110.github.io/textbook/chapters/basics/functions.html)

Ruddy already documents `fn x y => body` as `fn x => fn y => body`, and `f x y` as `(f x) y`. Use that equivalence to explain partial application. Also address a likely transfer mistake directly: Ruddy's `return` supplies the value of its enclosing `do` block; it does not exit a function. These are particularly valuable topics for the intended audience. [Ruddy grammar](../../docs/src/grammar.md)

### Small examples can carry substantial ideas

The book's functions section uses compact mathematical examples alongside specifications, inferred types, and semantic explanation. Small examples do not imply a beginner audience; the question is what reasoning they teach. [Functions](https://cs3110.github.io/textbook/chapters/basics/functions.html)

For Ruddy, retain short examples when they isolate a rule. Follow them with an exercise that requires prediction, explanation, implementation, or generalization. Do not force every concept into the contact scenario or make every section a sequence of file replacements. Provide runnable checkpoints and clearly explain how smaller fragments can be tried with Ruddy's available tools.

### Derive abstractions and practice choosing them

The map section starts with related recursive transformations and factors their differing operation into a function parameter. Ruddy can use the same teaching move: identify duplication, parameterize the varying behavior, and only then name the abstraction. [Map](https://cs3110.github.io/textbook/chapters/hop/map.html)

The options section motivates its representation through an operation with no result on empty input and considers other ways to represent that situation. For Ruddy, give `#Some` and `#None` a modeling purpose and explain what callers must handle. [Options](https://cs3110.github.io/textbook/chapters/data/options.html)

The variants section connects to enums while explaining where the analogy stops, including constructor terminology. The Ruddy guide should likewise identify specific false expectations from familiar languages, with structural types explained on their own terms. [Variants](https://cs3110.github.io/textbook/chapters/data/variants.html)

The fold section develops a common combination pattern and examines direction, readability, and short-circuiting. The lesson for Ruddy is to teach a choice among implementations, including the limits of an abstraction. [Fold](https://cs3110.github.io/textbook/chapters/hop/fold.html)

The data exercises include representation, implementation, edge cases, tests, and library lookup. For Ruddy, mix short reasoning questions with coding tasks and optional challenges. A successful exercise should reveal whether the reader can apply the rule to a new case. [Data exercises](https://cs3110.github.io/textbook/chapters/data/exercises.html)

### Adapt the teaching method to Ruddy

The guide should explain Ruddy's actual choices: structural fields and tags, immutable arrays, `do` sequencing, and algebraic effects. Its standard array combinators propagate callback effects in their types, offering a later connection between higher-order programming and effects. [Ruddy introduction](../../docs/src/index.md), [grammar](../../docs/src/grammar.md), [array implementation and signatures](../../std/array.rud)

Do not transplant OCaml syntax, linked-list cost assumptions, nominal record rules, or its module-system curriculum without checking Ruddy's corresponding behavior. The pedagogical model is conceptual progression and explicit reasoning; the language rules must come from this repository. No new Ruddy examples were executed during this textbook follow-up.

## What the primary sources actually do

| Source | Observed approach | Editorial implication for Ruddy |
| --- | --- | --- |
| [Go: Get started](https://go.dev/doc/tutorial/getting-started) | Names prerequisites, directories and files; supplies complete code, commands and output; progresses from printing to a dependency. | Document the whole edit/run loop. A reader should never need to infer where a snippet belongs. |
| [Rust: Introduction](https://doc.rust-lang.org/book/ch00-00-introduction.html) and [guessing game](https://doc.rust-lang.org/book/ch02-00-guessing-game-tutorial.html) | Separates concept chapters from projects. The early project introduces ideas in use and defers deeper treatments; intermediate versions run before the game is complete. | Give each new concept an immediate job. Provide a small useful program before comprehensive language coverage. |
| [Racket: Introduction with Pictures](https://docs.racket-lang.org/quick/) | Begins with visible values and shapes, then composes them into larger pictures. Names, functions and lists extend the same material. It deliberately demonstrates an argument-count error and shows how to find reference documentation. | Reuse a familiar example, make changes observable, and teach both recovery and lookup. |
| [Gleam tour](https://tour.gleam.run/) | Editable code compiles and runs inside the browser; output, errors and warnings are shown together. It also points confused readers to help. | Keep experimentation close to the explanation. An online playground would be a separate product investment; clear local run instructions are immediately useful. |
| [Elm introduction](https://guide.elm-lang.org/) | Identifies its application domain, shows an interactive counter, and acknowledges that unfamiliar code will be explained later. | Say what readers can do with the language before listing mechanisms. A preview can motivate without requiring mastery of every line. |
| [Diátaxis: Tutorials](https://diataxis.fr/tutorials/) | Recommends a concrete destination, small actions, frequent visible results, reliable steps and minimal digressions or alternatives. | Choose one path through the first session; link detailed explanations when they become relevant. |

These examples also expose tradeoffs. Rust's project quickly encounters several unfamiliar constructs; Racket's progression extends as far as macros and objects. Adopting their concrete progression does not require copying their scope. For Ruddy, prefer a smaller first lesson, then a tour with separate chapters. This is an editorial judgment based on the breadth of those source tutorials. [Rust project](https://doc.rust-lang.org/book/ch02-00-guessing-game-tutorial.html), [Racket tutorial](https://docs.racket-lang.org/quick/)

## What Ruddy already gets right, and where to improve

- **Keep the concrete data examples.** The landing page already connects `let`, functions, named fields and tagged alternatives to names and email addresses. They supply a coherent starting point. [Current introduction](../../docs/src/index.md)
- **Bring execution forward.** The landing page explains functional programming, static typing, inference, structural typing and effects before its getting-started links. Proposed change: a short purpose statement, an audience statement, and a prominent route to the first runnable program, followed by explanations grounded in that program. [Current introduction](../../docs/src/index.md)
- **Preserve the operational detail.** Hello World already names the file, working directory, command and output. Its next example introduces imports, numeric suffixes, nested calls, conditionals, recursion and sequencing together. Proposed change: organize the next lesson around a functional programming concept with a practical task. FizzBuzz can work as a compact comparison of control flow once application and sequencing are explained; its difficulty is the cluster of unfamiliar Ruddy constructs, not the underlying programming problem. [Hello World](../../docs/src/hello-world.md)
- **Make prerequisites match the selected path.** Installation currently builds the CLI with Rust; Node.js is needed for `ruddy run` with the JavaScript target. Put both requirements directly on the route that asks readers to run a program, while linking installation details. Treat editor configuration as a subsequent convenience. [Download](../../docs/src/download.md)
- **Offer a learning route beyond syntax lookup.** The current suggested reading order moves from Hello World to Grammar to Standard library. Proposed change: introduce a short guided tour between the first program and the reference pages. [Current reading order](../../docs/src/index.md#getting-started)

## Proposed guide structure

This is a proposed multi-section learning route, not a single-session promise. Setup and a first runnable program form a short preliminary checkpoint. The guide then develops concepts with worked reasoning and practice. The sequence adapts the selected book's progression to the features documented in [Ruddy's grammar](../../docs/src/grammar.md) and [standard library](../../docs/src/standard-library.md).

1. **Why Ruddy, and who this guide is for.** Intermediate programmers from non-functional languages; the perspective offered by functions, structural types, and effects.
2. **Expressions and evaluation.** Values, bindings, scope, conditionals, and block results. Explain how to determine an expression's value and type. Cover numeric conventions as needed.
3. **Functions.** Application, nested functions, partial application, recursion, and reading inferred function types. Derive equivalences rather than asking readers to memorize syntax.
4. **Data and matching.** Structs, tuples, tags, optional values, and arrays. Explain structural compatibility through examples that vary the shape of the data.
5. **Higher-order programming.** Begin with related transformations, identify their shared structure, then abstract with functions. Develop mapping, filtering, and folding with exercises.
6. **Effects and sequencing.** Connect pure transformations to operations, effect types, and handlers. Explain `do` and `return` precisely, then show why changing a handler is useful.
7. **Building larger programs.** Modules, public interfaces, diagnostics, and using the standard library. Finish with an integrating program and a route into reference documentation.

Keep real commands and expected output for runnable checkpoints, but do not require every explanatory fragment to be an entire replacement program. Introduce diagnostics and specification habits within the relevant chapters. Briefly identify the runtime's output handling at setup; develop the full effect model after readers can reason about functions and types. [Current IO explanation](../../docs/src/index.md#effects)

## Suggested opening

The following is draft copy, not published documentation:

> Ruddy is a functional language with structural types, type inference, and algebraic effects. This guide is for intermediate programmers coming from non-functional languages; no prior functional-programming experience is required.
>
> We'll begin with how Ruddy expressions evaluate and how functions compose. From there, we'll model data with structural types and tags, abstract repeated computations, and use effects to separate operations from their handlers. Each section develops the rules through examples and exercises so you can reason about programs of your own.

The language description follows the [README](../../README.md) and [current introduction](../../docs/src/index.md). The audience is the user's requirement; the teaching sequence is a proposed editorial choice informed by the selected book. Accompany this copy with the project's early-stage status, then a compact setup checkpoint. The guide still needs to be written and validated; the snippets below validate only individual building blocks.

## How to decide whether the introduction works

These are proposed acceptance checks, not research findings or completed validation:

- A new reader can identify the prerequisites, create the project, and reproduce the first output without unstated steps.
- Every runnable checkpoint contains a complete file or precise edit instructions, its command, and expected output.
- The reader can predict evaluation, explain type constraints, and implement an independent extension that combines the concepts.
- The deliberate error is clearly marked and followed by a successful recovery.
- Ruddy-specific and functional-programming terms appear beside concrete examples. Familiar programming concepts receive only the detail needed to explain differences in Ruddy.
- A reader can find the next lesson, API reference, and a real project help route; do not invent a community channel.
- Re-run the documented workflow against the release or commit being documented. Ask a few intended readers to attempt it without coaching, record where they hesitate, and revise those steps. This would supply local usability evidence that the surveyed tutorials alone cannot provide.

## Verified building blocks from the initial research

These small examples are retained as execution evidence, not as the revised lesson sequence. The standalone named-string example could illustrate a rule, but a literal-edit prompt alone does not assess the intended conceptual understanding. These checkpoints were executed with the existing `target/debug/ruddy` binary and working-tree standard library, using a local dependency path in a disposable project. They were not verified through a clean installation or a rebuilt/released compiler. No Rust test suite was run. This is execution evidence for the examples, not a full validation of a published onboarding flow.

After the installation prerequisites, create the project:

```sh
ruddy new hello
cd hello
```

Replace `src/main.rud` with:

```ruddy
let main = fn _ => println "Hello, Ada!"
```

Run `ruddy run`. The verified output is:

```text
Hello, Ada!
```

Draft explanation: “Ruddy calls `main` when the program starts. `fn _ =>` defines a function whose argument we don't use. `println` prints the text that follows it. Function application places the argument after the function.” These behaviors are also described in [Hello World](../../docs/src/hello-world.md).

An additional minimal binding example was verified during the initial research:

```ruddy
let name = "Ada"
let main = fn _ => println name
```

The verified output is `Ada`. Fold this syntax into a substantive example rather than making it a separate lesson.

Then replace the file with:

```ruddy
let display_name = fn person => person.name
let contact = { name: "Ada", email: "ada@example.com" }
let main = fn _ => println (display_name contact)
```

The verified output is again `Ada`. Draft explanation: “The braces group named fields into a value. `person.name` reads its `name` field. `display_name contact` calls our function with the contact; parentheses keep that call together as the argument to `println`. The function requires a `name` field without requiring the additional `email` field.” This expands the existing [introduction's example](../../docs/src/index.md).

For an explicitly marked error exercise, change `name: "Ada"` to `name: 42n` and run `ruddy check`. The observed exit status was 1, with this diagnostic at the `println` call on line 3:

```text
[type-mismatch] Error: text and a natural number cannot be the same type
```

Explain that `42n` is a natural number, while this output call needs text. Restore `"Ada"` and run again. Error formatting and location may change with the compiler; preserve the exact message only when maintained against the documented version. [Natural literals](../../docs/src/grammar.md#literals)

A subsequent complete checkpoint uses the alternative cases already introduced on the landing page:

```ruddy
let email_or_default = fn email => match email with
| #Some address => address
| #None => "support@example.com"
end

let main = fn _ => println (email_or_default (#Some "ada@example.com"))
```

The verified output is `ada@example.com`. Replace the last line with:

```ruddy
let main = fn _ => println (email_or_default #None)
```

The verified output is `support@example.com`. Use these branches as building blocks for an exercise about fallback policy. Both cases and the three earlier positive checkpoints exited successfully. The contact and fallback logic originate in the [current introduction](../../docs/src/index.md); the completed entry points and exercise sequence are proposed here.

No production documentation was changed by this research.
