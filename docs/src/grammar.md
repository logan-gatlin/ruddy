---
doc: true
---

# Grammar

Ruddy source files use the `.rud` extension.
This page describes how names, values, functions, and types are written, followed by effects and other advanced syntax.

## Source layout and names

A file contains [definitions](dictionary.md#definition) introduced by `let`, `type`, `effect`, `extern`, or `module`.
Spaces and newlines separate code; indentation does not determine its structure.
Definitions need no terminating punctuation.
Blocks use keywords `do` and `end`, [tuples](dictionary.md#tuple) use parentheses, [structs](dictionary.md#struct) use curly braces, and [arrays](dictionary.md#array) use square brackets.

An [identifier](dictionary.md#identifier) begins with a letter or underscore, followed by letters, digits, or underscores.
Names are case-sensitive, and letters may be Unicode.
The name `_` is reserved for ignoring a value or leaving a type unspecified.
Keywords such as `let`, `fn`, and `end` cannot be identifiers.
The words `when` and `where` have special meaning in type constraints but can be identifiers elsewhere.

A line comment starts with `--` and ends at the newline.
A block comment starts with `(*` and ends with `*)`; block comments can contain nested block comments.

## Literals

A [literal](dictionary.md#literal) writes a value directly in source code.

| Kind | Examples | Type |
| --- | --- | --- |
| Real number | `42`, `3.5`, `-2.5` | `Real` |
| Natural number | `42n` | `Nat` |
| Integer | `42i`, `-2i` | `Int` |
| Fixed-width natural number | `255n8`, `42n64` | `Nat8`, `Nat64` |
| Fixed-width integer | `-2i8`, `42i64` | `Int8`, `Int64` |
| String | `"Hello"` | `String` |
| Bool | `true`, `false` | `Bool` |
| [Unit](dictionary.md#unit) | `()` or `{}` | `()` |

The fixed-width suffixes support widths of `8`, `16`, `32`, and `64`.
Number literals use decimal digits; a decimal point requires digits on both sides.
The `n` and `i` suffixes require whole numbers, and natural numbers cannot be negative.

Quoted strings stay on one source line and support `\"`, `\\`, `\n`, `\r`, and `\t` for a quote, backslash, newline, carriage return, and tab.
Raw strings start each content line with `\\` and leave backslashes unchanged.
Consecutive raw string lines are joined with newlines; indentation before `\\` is excluded, and no newline is added after the last line.
For example, a raw string can hold a message spanning two lines:

```ruddy
let message =
  \\The upload is complete.
  \\The window can now close.
```

## Values and functions

A [binding](dictionary.md#binding) associates a name with a value.
An [expression](dictionary.md#expression) computes a value.
The form `let name = expression` creates a binding, and `let name: Type = expression` adds a [type annotation](dictionary.md#type-annotation).
For example, an annotation states that a retry limit is a natural number:

```ruddy
let retry_limit: Nat = 3n
```

The shorthand `_ = expression` means `let _ = expression`: it evaluates an expression and discards its result.
It is useful for effectful operations whose results are not needed, but also accepts expressions without effects.
Both forms work at file level, inside inline modules, and in `do` blocks, with the same effect rules.
The shorthand accepts a type annotation as `_ : Type = expression`, and attributes wherever the full `let` form permits them.

A function uses `fn`, one or more parameter names, `=>`, and its body expression.
The parameter `_` ignores its argument.
Functions with several parameters receive their arguments one at a time: `fn x y => body` means `fn x => fn y => body`.
For example, a function can calculate a rectangle's area:

```ruddy
let area = fn width height => width * height
```

Function calls place arguments after the function, separated by spaces.
The call `area 3 4` means `(area 3) 4`, while `area (3 + 1) 4` groups a calculation as the first argument.
Parentheses also group an `if`, `match`, `handle`, or `do` expression when it is passed as an argument.
A [tag](dictionary.md#tag) can carry one value, so `#Some (area 3 4)` groups the call that supplies that value.

## Structs, tuples, arrays, and tags

A [struct](dictionary.md#struct) uses braces and comma-separated fields in the form `name: expression`.
A field name can also be a quoted string or a nonnegative decimal position.
A dot accesses a field, as in `contact.email`, `contact."display name"`, or `pair.0`.
For example, a struct can group contact details:

```ruddy
let contact = { name: "Ada", email: "ada@example.com" }
```

A [tuple](dictionary.md#tuple) uses parentheses and commas, as in `("Ada", 3n)`.
Positions start at zero.
A single-element tuple needs a comma: `(value,)` is a tuple, while `(value)` groups an expression.

An [array](dictionary.md#array) uses brackets, as in `["Ada", "Lin"]` or `[]`.
All elements have a compatible element type.
Structs, tuples, and arrays allow a trailing comma.

A [spread](dictionary.md#spread) uses `..` to include fields or elements from another value.
A struct allows one spread, written last; explicitly written fields replace fields with the same name.
An array allows several spreads in any position.
For example, a spread adds a recipient while retaining the existing recipients:

```ruddy
let recipients = ["ada@example.com"]
let all_recipients = [..recipients, "lin@example.com"]
```

A [tag](dictionary.md#tag) begins with `#`, as in `#None` or `#Some "Ada"`.
A quoted tag such as `#"not found"` permits spaces in its name.
There is no space between `#` and its name or opening quote.

## Operators

The following table lists expression operators from tightest to loosest grouping.

| Form | Meaning | Grouping when repeated |
| --- | --- | --- |
| `value.field` | Field access | Left |
| `function argument` | Function call | Left |
| `-`, `not`, `mut`, `~` before an expression | Negation, Bool negation, [mutable cell](dictionary.md#mutable-cell) creation, mutable cell reading | Right |
| `*`, `/` | Real multiplication and division | Left |
| `+`, `-` | Real addition and subtraction | Left |
| `and` | Bool conjunction | Left |
| `xor` | Bool exclusive disjunction | Left |
| `or` | Bool disjunction | Left |
| `\|>` | Pass the left value to the right function | Left |
| `:=` | Cell assignment | Right |

Parentheses override this grouping.
The arithmetic operators work on `Real`; integer arithmetic and comparisons use functions such as `std::nat::add` and `std::nat::less_than`.
The form `value |> f |> g` means `g (f value)`.
A minus immediately followed by digits is part of a number literal, so subtraction should have spaces around `-`, as in `total - 1`.

## Blocks and choices

A `do` block contains local `let` bindings or `_ = expression` shortcuts followed by an optional `return expression` and a closing `end`.
The expression after `return` is the block's value; without `return`, the value is `()`.
The keyword `return` does not return from a function; it supplies the value of its enclosing `do` block.
The shorthand `_ = expression` evaluates an operation for its effects while discarding its result.
For example, a block can print a message before returning a status:

```ruddy
let notify = fn message => do
  _ = std::io::print message
  return #Sent
end
```

An `if` expression requires a Bool condition, a `then` expression, an `else` expression, and `end`.
An `else if` chain shares one final `end`.
For example, a condition selects a status message:

```ruddy
let status = fn ready => if ready then "Ready" else "Waiting" end
```

## Patterns

A [pattern](dictionary.md#pattern) describes a value's shape and can bind names to its parts.
Patterns appear after `let` and in [pattern matching](dictionary.md#pattern-matching) with `match`.

| Pattern | Meaning |
| --- | --- |
| `name` | Bind the value to a name |
| `_` | Ignore the value |
| `3n`, `"Ada"`, `true`, `()` | Match a literal value |
| `{ name, email: address }` | Match fields and bind their values |
| `{ name, .. }` | Match a field while allowing additional fields |
| `(first, second)` | Match tuple positions |
| `[]`, `[first, second]` | Match an exact array length |
| `[first, ..rest]`, `[first, .., last]` | Match an array with a variable number of middle elements |
| `#Some value`, `#None` | Match a tag and any value it carries |

A struct pattern's `..` must be last and cannot bind a name.
An array pattern permits at most one `..`, optionally followed by a name for the remaining elements.
Patterns can contain other patterns.
Function parameters written after `fn` are names or `_`; matching their structure uses `match` instead.

A `match` expression uses `match expression with`, arms separated by `|`, and a closing `end`.
Each arm has the form `pattern => expression`, and the first `|` is optional.
For example, matching an array selects its first recipient or a default:

```ruddy
let first_recipient = fn recipients => match recipients with
| [first, ..] => first
| [] => "support@example.com"
end
```

The shorthand `fn | pattern => expression | pattern => expression` defines a function that matches its argument directly, with no closing `end`.
Its leading `|` is required; parentheses delimit a nested shorthand when more arms follow outside it.

## Types

A type definition uses `type Name = Type`.
[Type variables](dictionary.md#type-variable) begin with `'`, and parameters follow the name being defined.
For example, a type can describe an optional value of any supplied type:

```ruddy
type Optional 'a = #Some 'a | #None
```

The type `Optional String` supplies `String` as the argument for `'a`.
Parentheses group a type argument that itself contains an application, as in `Optional (Optional String)`.

| Type form | Meaning |
| --- | --- |
| `String`, `Nat`, `Contact` | A type name |
| `'a` | A type variable |
| `_` | A type left for inference |
| `{ name: String, age: Nat }` | A struct type |
| `(String, Nat)` | A tuple type |
| `[String]` | An array type |
| `#Some String \| #None` | A [sum type](dictionary.md#sum-type) with two cases |
| `\|` | A sum type with no cases |
| `String -> Nat` | A function type |
| `mut 'r String` | A mutable cell type with [region](dictionary.md#region) `'r` |

Function arrows group to the right: `String -> Nat -> String` means `String -> (Nat -> String)`.
A tag's payload type needs parentheses when it contains an application or arrow, as in `#Some (Optional String)`.

### Open types and constraints

A [row](dictionary.md#row) describes a collection of struct fields, sum cases, or effects.
A final `..` permits additional entries, and `..'r` names those entries so another part of the type can refer to them.
For example, an open struct type accepts a contact with additional fields:

```ruddy
let email: { email: String, .. } -> String = fn contact => contact.email
```

Struct fields and sum cases share a row kind. A parameter used as a row tail
accepts the underlying row of either a struct or a sum, including through type aliases:

```ruddy
type Sum 'r = | ..'r
type Struct 'r = { ..'r }
type Choice = Sum { a: (), b: () }
type Product = Struct (#A () | #B ())
type Both 'r = { product: { ..'r }, choice: | ..'r }
```

`Choice` means `#a () | #b ()`, and `Product` means `{ A: (), B: () }`.
Labels, payload types, presence conditions, and open tails are preserved.
In `Both`, the struct and sum share the same row, so constraints from either use
apply to both. A row must exclude every label already named beside any of its uses.
Empty structs and empty sums supply the same empty row.

Struct and sum values remain distinct types. Row extraction happens at row
parameters; it does not convert values. A parameter cannot stand for both a
whole type and a row, and effect rows remain a separate kind.

A [presence variable](dictionary.md#presence-variable) controls whether a field, case, or effect is present.
A `where` clause constrains those variables with `not`, `and`, `or`, `=`, and `!=`, in that order from tightest to loosest grouping.
Semicolons separate multiple constraints, and parentheses group them.

| Form | Meaning |
| --- | --- |
| `{ email when 'p: String }` | A field whose presence is controlled by `'p` |
| `#Some (when 'p) String` | A case whose presence is controlled by `'p` |
| `!Log (when 'p)` | An effect whose presence is controlled by `'p` |
| `when _` | A presence left unnamed |
| `{ \email, .. }` | An open struct type excluding a field |
| `\#None \| ..'r` | A sum row excluding a case |
| `\!Log + ..'e` | An effect row excluding an effect |
| `where 'p = 'q` | Require two presences to agree |
| `where 'p != 'q` | Require two presences to differ |

## Effects and handlers

An [effect](dictionary.md#effect) definition uses `effect Name = Input -> Output` for one unnamed operation, or braces containing named operation signatures.
An operation is referenced as `!Name` or `!Name.operation` and called like a function.
For example, a clock effect provides one operation for reading the current time:

```ruddy
effect Clock = { now: () -> Nat }
```

Effect definitions can take parameters, as in `effect Ask 'a = () -> 'a`.
The form `effect IO = !Log + !Clock` combines effects, while `effect Marker` defines an effect with no operations.

A function type lists its effects after `+`, as in `() -> Nat + !Clock`.
Further effects are separated by `+`, an open effect row ends in `+ ..'e`, and `+ |` explicitly permits no effects.
Effects attach to the innermost arrow unless parentheses group that arrow: `A -> (B -> C) + !Log` puts `!Log` on the outer call.

An [effect handler](dictionary.md#effect-handler) uses `handle expression with`, arms separated by `|`, and `end`.
An operation arm has the form `!Effect.operation argument => expression`; an optional `return name => expression` arm transforms the handled expression's normal result.
The first `|` is optional, and arm arguments are names or `_`.
An operation arm's result normally resumes the operation; `raise expression` instead ends the handled computation with that value.
The use of `raise` must be inside a handler arm and cannot cross into a nested `fn`.
For example, a handler supplies a fixed clock value:

```ruddy
effect Clock = { now: () -> Nat }
let timestamp = handle !Clock.now () with
| !Clock.now _ => 100n
end
```

## Mutable cells

A [mutable cell](dictionary.md#mutable-cell) is created with `mut expression`, read with `~cell`, and updated with `cell := expression`.
Assignment returns the value written.
For example, a cell can hold a running total:

```ruddy
let total_with_fee = fn subtotal => do
  let total = mut subtotal
  _ = total := ~total + 5
  return ~total
end
```

The type `mut 'r Real` describes a cell containing a `Real` in [region](dictionary.md#region) `'r`.
Reading and writing cells are tracked by the effect `!mut 'r`.

## Modules, attributes, and foreign values

A [module](dictionary.md#module) groups names under a shared name.
An inline module uses `module Name =`, its contents, and `end`.
The form `module name` loads the module from a separate file.
A path uses `::` between module names, as in `std::io::print`; effect paths put `!` before the final effect name, as in `std::io::!IO.write`.
Paths normally resolve their first name from the surrounding lexical scope. A leading `::` starts at the bundle root instead, so `::std::io::println` names the same definition in every nested scope.
For example, an inline module groups application defaults:

```ruddy
module Defaults =
  let retry_limit = 3n
end
```

A `using` statement makes existing declarations available under shorter names.
`using Defaults` binds the module name; `using Defaults::*` brings its declarations
into scope. Use `as` to rename a binding and braces to group imports:

```ruddy
module Defaults =
  type Count = Nat
  let retry_limit = 3n
end

using Defaults::{self as defaults, Count, retry_limit as retries}
let limit: Count = retries
let qualified = defaults::retry_limit
```

Groups can nest and include globs, such as `using App::{Settings::{self, *}}`.
An import brings in every matching value, type, effect, and module namespace.
Write an effect's declaration name in the import, then use its usual `!` spelling.

Module-level imports apply throughout the containing module and its nested
scopes, including before the statement. They can refer to later imports.
Inside a `do` block, imports apply only from their statement onward:

```ruddy
module Defaults =
  let retry_limit = 3n
end

let limit = do
  using Defaults::retry_limit as retries
  return retries
end
```

`bundle::` starts at the current bundle's root, `self::` at the containing module,
and `super::` at its parent. Repeat `super::` to climb further; climbing above the
root is an error. These prefixes also work in ordinary value, type, and effect
paths. Importing an anchor itself requires an alias, such as
`using bundle as root` or `using super as parent`.

Explicit imports cannot duplicate an explicit import or declaration in the same
namespace and scope. Explicit names override glob imports. Two globs exposing
different declarations under the same name are ambiguous only when that name is
used. Inner scopes can shadow outer names, and imports override the standard
prelude. An invalid import is an error even when unused.

Imports do not add bundle exports or qualified members to their containing
module. If `A` imports `B::item`, that alone does not make `A::item` available.
Likewise, `using A::*` imports A's accessible declarations, not A's imported
names. Existing bundle-private access rules still apply.

An [attribute](dictionary.md#attribute) precedes a definition as `@key` or `@key literal`.
Several attributes can appear together, and their values may contain literal structs, tuples, arrays, or tags.
An omitted value means `()`.
For example, an attribute can record a version on a value:

```ruddy
@since "1.0"
let retry_limit = 3n
```

Row formula projection collects at most 256 product terms by default. Exceeding
that limit is a compilation error. Set `@max_sat_terms <nat>` on a value
definition to use a different maximum for each projection while checking that
definition, including its nested bindings and annotations:

```ruddy
@max_sat_terms 1024n
let choose = fn value => match value with
  | {left} => left
  | {right} => right
end
```

The value must be a natural literal (with the `n` suffix) that fits the
compiler's address size. The maximum is inclusive, can be raised or lowered,
and does not change the budget of other definitions or called functions.
A zero maximum permits no product terms, including the empty product for
`always`. Terms are counted during projection, before minimization; this is
independent of the size of the printed `where` clause. A larger maximum may
require more compilation time and memory.

A [foreign value](dictionary.md#foreign-value) uses `extern name: Type = "target expression"` and requires an explicit type.
Within an `extern` type, `fn(A, B) -> R` describes a foreign function that receives two arguments in one call.
Ruddy code still calls it as `function a b`.
These foreign function types can describe nested callbacks, and attributes can precede them to specify foreign calling behavior.
For example, a JavaScript foreign function can expose a string's uppercase conversion:

```ruddy
extern uppercase: fn(String) -> String = "text => text.toUpperCase()"
```
