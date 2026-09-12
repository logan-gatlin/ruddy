---
doc: true
---

# Standard library

The [Ruddy Book](index.md) introduces these modules as their concepts become relevant.
The [generated API reference](std/bundle.md) contains the complete public declarations.

The [standard library](dictionary.md#standard-library) is available through the dependency name `std` by default.
It provides common types, functions, and effects.
The [grammar](grammar.md#modules-attributes-and-foreign-values) describes qualified paths and `using` statements.

## Prelude

The [prelude](dictionary.md#prelude) makes the declarations in `std::prelude` available without an explicit import.
Local declarations and explicit imports can shadow these names.

| Names | Purpose |
| --- | --- |
| `print`, `println` | Write a string to standard output, without or with a trailing newline |
| `eprint`, `eprintln` | Write a string to standard error, without or with a trailing newline |
| `drop` | Discard a value and return `()` |
| `IO` | An alias for the output effect, used as `!IO` in types |
| `FileSystem` | An alias for the filesystem effect, used as `!FileSystem` in types |
| `Option 't` | An optional value: `#Some 't` or `#None` |
| `Maybe 't` | A sum containing `#Nil` and the cases supplied by the row `'t` |
| `Result 'some 'error` | A result: `#Some 'some` or `#Error 'error` |
| `Fallible 'error 'rest` | A sum containing `#Error 'error` and the cases supplied by the row `'rest` |
| `Order` | A comparison result: `#Less`, `#Equal`, or `#Greater` |

The printing functions return `()` and perform `!IO`.
For example, a function can declare its output effect using prelude names:

```ruddy
let greet: String -> () + !IO = fn name => println name
```

Primitive types such as `Bool`, `String`, `Nat`, `Int`, and `Real` are built into the language and remain available without the standard library.
Other library modules, including `nat`, `str`, and `array`, are not prelude names.
For example, an explicit import makes string conversion and natural-number arithmetic available through short module paths:

```ruddy
using std::{nat, str}

let next_label = fn number => str::from_nat (nat::add number 1n)
```

Without those imports, the paths are `std::str::from_nat` and `std::nat::add`.
The same import style is used in the [FizzBuzz example](hello-world.md#try-fizzbuzz).

## Modules and naming

| Module paths | Contents |
| --- | --- |
| `std::nat`, `std::int`, `std::real` | Arithmetic, conversions, and comparisons |
| `std::boolean` | Operations on `Bool` values |
| `std::str` | String operations and conversions |
| `std::array` | Operations on immutable arrays |
| `std::option`, `std::result` | Operations on optional values and results |
| `std::order` | Comparison results, comparison functions, and derived comparisons |
| `std::tuple`, `std::function` | Tuple and function helpers |
| `std::cell` | Mutable cell operations |
| `std::io`, `std::fs`, `std::process` | Output, filesystem operations, and process operations |
| `std::types`, `std::effects` | Additional types and effect definitions |
| `std::any`, `std::ffi` | Runtime type information and foreign-value conversion |

Array and string lengths use `array::len` and `str::len` after importing those modules.
The `order` module provides `Ordering`, the type of a comparison function, while `Order` describes its result.
The numeric and string modules expose `compare` and comparisons such as `equal`, `less_than`, and `greater_than_or_equal`.
`order::PartialOrder` adds `#Unordered` to the three total-order results. `order::compare` compares values structurally using reflection, and supplies the six infix comparison operators. `real::compare` is partial too: NaNs are unordered and signed zeros are equal. Use `real::total_compare` when sorting requires a total ordering.
Opened existential types need a mirror carried by their package before they can be compared. This also applies when the opened type occurs inside a function type; comparisons retain reflection’s existing evidence requirements.
For example, a comparison can check whether a natural number is below a limit:

```ruddy
using std::nat

let below_limit = fn value => nat::less_than value 100n
```

The output effect is `std::io::!IO`, also available as `!IO` through the prelude.
Its operations are `std::io::!IO.write` and `std::io::!IO.write_error`.
The prelude aliases name effects in types but cannot be used to perform or handle operations.
An explicit `using std::io::IO` import permits `!IO.write` and `!IO.write_error` in calls and handler arms.
The functions `std::io::consumer` and `std::io::consumerln` repeatedly accept strings for output; `econsumer` and `econsumerln` write to standard error.

## Configuring the library

Omitting `std` from `[dependencies]` in `Ruddy.toml` selects the default standard library.
An explicit path selects another standard library, while `std = false` disables the dependency and its prelude.
For example, a project that needs no standard library can disable it:

```toml
[dependencies]
std = false
```
