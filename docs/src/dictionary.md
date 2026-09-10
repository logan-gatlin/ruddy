---
doc: true
---

# Dictionary

To keep usage consistent, this file lists terms used in documentation and their meanings.
Definitions are written like this:

## Word
Its definition.
Keep this to three lines at the most.
After the definition, inline Markdown links reference pages that are primary sources of information on the topic.
References are omitted when no such page exists.

`[Topic](topic.md)`

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
A value written directly in source code, such as a number, string, or Boolean.

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

## Sum type
A type describing alternative cases identified by tags, each of which can carry a value.

[Grammar](grammar.md#types)

## Row
A collection of struct fields, sum cases, or effects, optionally allowing additional entries through `..`.

[Grammar](grammar.md#open-types-and-constraints)

## Presence variable
A variable that controls whether a struct field, sum case, or effect is present.

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
