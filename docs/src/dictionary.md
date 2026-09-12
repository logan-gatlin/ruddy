---
doc: true
---

# Dictionary

This appendix defines the terminology used throughout [The Ruddy Book](index.md).
Each definition links to a chapter or reference page that develops the concept.

## Word
A term used consistently with its definition in this dictionary.

## Command-line interface
A tool controlled by commands entered in a terminal, abbreviated CLI.
Ruddy's CLI creates, checks, builds, runs, and formats projects through the `ruddy` command.

[Hello world](hello-world.md)

## Standard library
The library available by default as `std`, providing common types and functions such as console output.

[Standard library](standard-library.md)

## Prelude
The declarations in `std::prelude` that are available without an explicit import or module qualification when the standard library is enabled.

[Standard library](standard-library.md#prelude)

## Definition
A top-level `let`, `type`, `effect`, `extern`, or `module` that defines a value, type, effect, foreign value, or module, respectively.
The shorthand `_ = expression` is also a definition, equivalent to `let _ = expression`.

## Functional language
A language in which programs are built by combining functions, and functions can be passed as arguments and returned as values.

## Static typing
Checking that values are used consistently with their types before a program runs.

## Type inference
Determining types from code without requiring explicit types for every value.

## Struct
A value containing named fields, each of which holds a value.

## Type compatibility
Whether a value of one type can be used where another type is expected.
Compatibility in one direction does not imply compatibility in the other.

## Structural typing
Determining [type compatibility](#type-compatibility) from the structure of types rather than the names of their [definitions](#definition).

## Tag
A label beginning with `#` that identifies a case and can carry a value.

## Pattern matching
Selecting an expression by comparing a value against patterns that describe its shape and can bind names to its parts.

## Effect
A description of an operation, or a group of operations, whose behavior is supplied by an [effect handler](#effect-handler) and whose use is tracked in function types.

## Effect handler
Code that supplies the behavior of [effect](#effect) operations within the expression it handles.

## Unit
The value `()`, used when no information needs to be supplied or returned.
Its type is also written `()`.
The unit is equivalent to a struct with no fields (`{}`).

## Identifier
A name written in source code to refer to a value, type, effect, or module.

[Grammar](grammar.md#source-layout-and-names)

## Literal
A value written directly in source code, such as a number, string, or `Bool`.

[Grammar](grammar.md#literals)

## Binding
An association between a name and a value.

[Grammar](grammar.md#values-and-functions)

## Expression
Code that computes a value.

[Grammar](grammar.md#values-and-functions)

## Type annotation
An explicit type written for a value.

[Grammar](grammar.md#values-and-functions), [Type syntax](grammar.md#types)

## Tuple
A struct whose fields are numbered positions starting at zero.

[Grammar](grammar.md#structs-tuples-arrays-and-tags)

## Array
An immutable sequence of values with a shared element type.

[Grammar](grammar.md#structs-tuples-arrays-and-tags)

## Spread
The use of `..` in a struct or array expression to include fields or elements from another value.

[Grammar](grammar.md#structs-tuples-arrays-and-tags)

## Pattern
A description of a value's shape that can bind names to its parts.

[Grammar](grammar.md#patterns)

## Type variable
A name beginning with `'` that stands for a type or another parameter used in a type annotation or definition.

[Grammar](grammar.md#types)

## Hidden type
A type written `hide 'a => T` whose variable stands for one particular type chosen where a value is made and kept from the value's consumers, who can open it in a `match` arm.

[Grammar](grammar.md#types)

## Mirror
A value of type `Mirror T` that is authentic evidence for the type `T`. Only the compiler makes one, at a position whose type it knows; the standard `reflect` module reads it as data and decides whether two mirrors are one type.

## Sum type
A type describing alternative cases identified by tags, each of which can carry a value.

[Grammar](grammar.md#types)

## Row
A collection of struct fields, sum cases, or effects, optionally allowing additional entries through `..`.
A named tail such as `..'rest` can preserve those entries across inputs and outputs.
Struct and sum rows share a kind; effect rows have a separate kind.

[Grammar](grammar.md#open-types-and-constraints)

## Presence variable
A type-level boolean describing whether a struct field, sum case, or effect belongs to a type.
Presence variables and constraints can be inferred from program shape, and one variable can connect entries across structs, sums, and effects.

[Rows and presence inference](book/rows.md#presence-is-inferred-from-program-shape)

[Grammar](grammar.md#open-types-and-constraints)

## Mutable cell
A value that holds another value and allows that stored value to be read and replaced.

[Grammar](grammar.md#mutable-cells)

## Region
A type-level identity used to associate mutable cells with their read and write effects.

[Grammar](grammar.md#mutable-cells)

## Module
A group of names accessible through a shared name using `::`.

[Grammar](grammar.md#modules-attributes-and-foreign-values)

## Attribute
Metadata written with `@` before a definition or foreign function type.

[Grammar](grammar.md#modules-attributes-and-foreign-values)

## Foreign value
A value supplied by the target environment and made available to Ruddy through `extern`.

[Grammar](grammar.md#modules-attributes-and-foreign-values)

## Primitive contract
One operation of the portable table the standard library's `extern`
declarations name, as `$prim.<module>.<name>`. Every target implements the
same table, so a program built from these runs wherever Ruddy runs.

[Grammar](grammar.md#modules-attributes-and-foreign-values)

## Dynamic semantics
The rules describing how expressions evaluate and operations are performed.

[Expressions](book/expressions.md#evaluation-and-types)

## Static semantics
The rules describing which programs are accepted before execution, including their type constraints.

[Expressions](book/expressions.md#evaluation-and-types)

## Lexical scope
The association of names with bindings determined by the structure of source code.

[Blocks and scope](book/expressions.md#blocks-and-scope)

## Shadowing
Introducing a binding whose name makes an outer binding of the same name unavailable within the inner scope.
It does not update the outer binding's value.

[Bindings and mutation](book/expressions.md#bindings-and-mutation)

## Function application
Calling a function with an argument, written by placing the argument after the function.

[Functions](book/functions.md#application-and-grouping)

## Closure
A function together with the bindings from its lexical scope that it retains for later calls.

[Functions that return functions](book/functions.md#functions-that-return-functions)

## Currying
Representing a function of several arguments as nested functions that each accept one argument.

[Functions that return functions](book/functions.md#functions-that-return-functions)

## Partial application
Applying some arguments of a curried function to obtain a function that accepts the remaining arguments.

[Functions that return functions](book/functions.md#functions-that-return-functions)

## Higher-order function
A function that accepts or returns a function.

[Higher-order programming](book/higher-order.md)

## Pipeline
A sequence of applications written with `|>`, which supplies its left value as the argument to its right function.

[Selecting and combining](book/higher-order.md#selecting-and-combining)

## Fold
A computation that combines sequence elements with an accumulated state through a supplied step function.

[Selecting and combining](book/higher-order.md#selecting-and-combining)

## Immutable
Describing a value whose contents cannot be changed after construction.
An operation can produce a new value without changing the original.

[Modeling data](book/data.md#fields-and-immutable-updates)

## Bundle
A project described by `Ruddy.toml`, with an identity, root source, and executable or library kind.
Libraries can be dependencies of other bundles.

[Organizing programs](book/modules.md#moving-a-module-into-a-file)

## Codec
A pair of operations for encoding and decoding values through a representation protocol.

[Structured data](book/external-data.md), [Codec reference](std/codec.md)


## Value
The result of a normally completed expression, requiring no further evaluation of that expression.

[Values and expressions](book/expressions.md)

## Evaluation
The computation of an expression's result according to the language's dynamic semantics.

[Evaluation and types](book/expressions.md#evaluation-and-types)

## Pure function
A function with no externally observable effects whose result depends only on its inputs and immutable captured values.
Locally handled operations or isolated local mutation can be used inside a pure function.
Purity does not imply termination.

[Effects](book/effects.md), [State](book/state.md)

## Recursion
Defining a computation in terms of calls to itself, directly or through other functions.

[Recursion](book/functions.md#recursion)

## Predicate
A function whose result is a boolean describing whether an input satisfies a condition.

[Filtering](book/higher-order.md#selecting-and-combining)

## Accumulator
The state carried from one step of a fold to the next.

[Folding](book/higher-order.md#selecting-and-combining)

## Callback
A function supplied to another operation for that operation to invoke.

[Higher-order programming](book/higher-order.md), [Foreign callbacks](book/interoperability.md#callbacks-and-completion)

## Host
The environment executing foreign code and supplying host values, such as JavaScript on Node.

[Interoperability](book/interoperability.md)

## Snapshot
An inert copy of supported host data that can be read without further observation of the live host value.

[Checked data conversion](book/interoperability.md#checked-data-conversion)

## Schema
The agreed structure and interpretation of a stored or transmitted representation.

[Representations and schemas](book/reflection.md#representations-and-schemas)


## Tail call
A function call whose result is returned directly, with no further computation waiting for that result.
Ruddy can continue with the called function without retaining additional pending return work.

[Tail calls and loops](book/functions.md#tail-calls-and-loops)

## Structural sharing
Reusing unchanged parts of an immutable data structure in several values or versions instead of copying those parts.

[Immutable array trees](book/data.md#arrays-are-immutable-trees)

## Rethrowing an effect
Performing an effect operation from its own handler arm so that an outer handler receives it.
This is distinct from `raise`, which ends a handled computation.

[Composing log handlers](book/effects.md#rethrowing-and-composing-log-handlers)

## Row polymorphism
Allowing a function or type to work with different rows while preserving relationships expressed by shared row variables.
For example, the same `..'rest` in a function's input and output preserves the additional field names and types.

[Rows and subtyping](book/rows.md#from-subtyping-to-row-polymorphism)

## Subtyping
A relationship that allows a value of one type to be used where another type is expected.
Nominal subtyping uses declared relationships; structural subtyping uses the types' structure.
Ruddy's open rows provide explicit room for additional entries, while a closed struct type does not silently accept extra fields.

[Rows and subtyping](book/rows.md#from-subtyping-to-row-polymorphism)
