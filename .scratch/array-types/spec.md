# Immutable homogeneous arrays

## Source surface

- Array literals are `[]`, `[x]`, `[x, y]`, and `[x, y,]`; a trailing comma is optional.
- Array types are written `[T]`.
- Arrays are homogeneous. Literal elements must unify to one element type.
- The empty literal `[]` infers a generalized `['a]` when its binding is generalized.
- Array length is a runtime property and is not part of the type.
- Array spread syntax and array patterns are not included.
- Compiler and debugger printers render array types and values with bracket syntax.

## Standard API

The initial API is array-first and uses `Nat` indices:

```ruddy
array::len  : ['a] -> Nat
array::get  : ['a] -> Nat -> Option 'a
array::set  : ['a] -> Nat -> 'a -> Option ['a]
array::push : ['a] -> 'a -> ['a]
```

`get` and `set` return `#None` for an out-of-bounds index. `set` and `push`
return new arrays and leave their inputs unchanged. `pop`, concatenation, slicing,
mapping, folding, equality, conversions, and computed-index syntax are deferred.

## Runtime representation

- Arrays use a private immutable persistent vector implemented as a fixed-width
  32-way trie with a dense tail of at most 32 elements.
- The representation is not observable or guaranteed. Observable guarantees are
  immutability, structural sharing, and the performance contract below.
- Literal evaluation may use a private transient builder, but constructed arrays
  are observably immutable.
- Arrays are not supported in extern signatures in this initial version. Such
  signatures receive a focused diagnostic rather than exposing the trie or
  performing an implicit linear conversion.

## Performance contract

- `len`: O(1)
- `get` and `set`: O(log₃₂ n)
- `push`: O(log₃₂ n), with structural sharing
- Literal construction: O(n)
- `set` and `push`: O(log₃₂ n) newly allocated storage

## End-to-end coverage

The feature covers the lexer/parser, AST and normalized IR, inference,
representation-aware lowering, artifact serialization and validation, JavaScript
backend/runtime, tree-sitter grammar and corpus, debugger views, source/type
printers, standard library surface, focused diagnostics, and tests at the public
parsing, inference, execution, printing, and tooling seams.
