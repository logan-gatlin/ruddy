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
| `display` | Convert any value to indented diagnostic text |
| `debug` | Convert any value to single-line diagnostic text |
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
For example, an explicit import makes value formatting and natural-number arithmetic available through short module paths:

```ruddy
using std::{nat, str}

let next_label = fn number => str::display (nat::add number 1n)
```

Without those imports, the paths are `std::str::display` and `std::nat::add`.
`display` is also available directly from the prelude; `next_label 2n` returns `"3n"`.
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
| `std::hash` | Pure, seeded structural hashing with errors for unsupported values |
| `std::map` | Persistent key/value maps with forgiving and diagnostic operations |
| `std::set` | Persistent sets, membership, and set algebra |
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

`hash::hash` produces a `Result Nat64 hash::Error` through the same typed reflection
views. For equal values at a shared static type, successful hashes agree when
the seed agrees. `hash::hash_seeded` accepts a collection's seed, and
`hash::hash_with` also accepts an explicit mirror. Hashes are ordinary
noncryptographic collection hashes, with no promise of stability across releases
or resistance to deliberate collisions. A collection must still compare keys
after matching hashes.

Hashing rejects NaNs and encountered opaque values, including cells, functions,
mirrors, and hidden packages. Errors carry a root-to-leaf field, case, and array
index path. An empty array or an inactive unsupported sum case does not fail.
Signed zeros hash equally. Open a hidden package that carries a mirror to hash
its value explicitly.

The implementation threads an immutable FNV-1a 64 accumulator through reflection.
This keeps the public operation pure and the traversal explicit. Local mutation
could preserve that same public contract, but is unnecessary for the first
implementation. A hashing effect could separate traversal from interchangeable
handlers; it is deferred until there is a consumer for that protocol.

`map` and `set` use a shared persistent hash array mapped trie (HAMT). Updates
copy the changed path and share unchanged branches. Earlier versions retain
their associations; values containing cells still share those mutable cells.
A cell may hold the current collection version, using the ordinary mutation
effect, while the collection operations themselves remain pure.

Ordinary key operations treat an unhashable key as absent. `get` returns
`#None`, `contains` returns `false`, and insertion or removal leaves the original
collection unchanged. `from_array` skips invalid entries and keeps valid ones.
Map construction uses the last accepted value for an equal key. Map values do
not need to be hashable.

The corresponding `try_*` functions return `Result` with `hash::Error`.
`map::try_get` distinguishes a hashing error from a valid missing key
(`#Some #None`). Strict constructors stop at the first invalid entry and return
no partial collection; the error path begins with its array index, followed by
the path within the key or set element.

Iteration order is unspecified. Use `map::equal_by` or `set::equal` to compare
contents independently of insertion history. Generic reflection sees their
hidden package rather than providing collection equality or collection hashing.
Set algebra and value transforms reuse the hashes of already accepted keys.

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
