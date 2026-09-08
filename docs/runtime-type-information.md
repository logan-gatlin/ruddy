# Runtime type information and JavaScript

`Any` stores a Ruddy value together with its static structural type. Ordinary
values keep their existing representation. `JsValue` instead stores an arbitrary
JavaScript value, preserving its identity without claiming a Ruddy payload type.

```ruddy
let stored: Any = std::any::upcast [1n, 2n]
let recovered: std::Option [Nat] = std::any::downcast stored
let mismatch: std::Option [Int] = std::any::downcast stored
```

A downcast returns `#Some` with the original payload exactly when the types are
structurally equal, and `#None` otherwise. Aliases do not introduce identity.
Array element types, record fields, sum payloads, numeric widths, and signedness
participate in equality. Recursive types are compared as finite graphs.

Generic helpers need no new annotations:

```ruddy
let box: 'a -> Any = fn value => std::any::upcast value
extern identity: 'a -> 'a = "value => value"
```

The compiler infers runtime representation requirements separately from effects.
Portable schemes record them; a use supplies descriptors for its chosen types.
Each arrow receives only its inferred descriptor arguments, before its effect
evidence and visible argument. Constructing a function does not call its body.
Initializers execute once in their lexical environment; evidence needed by that
execution must already be available there. Returned closures can capture evidence
or receive it on a later call, as indicated by their own inferred interface.

Higher-order interfaces quantify callback demands. Adapters reconcile different
hidden argument layouts through aliases, aggregates, patterns and branches.
When an abstract descriptor slot is used at a compound type, its constituent
descriptors construct that slot; callback adapters can project component evidence.
Values crossing `Any`, a generic native position, an effect operation or mutable
callable storage seal their callable convention by capturing needed descriptors.
This does not attach type identity to the value or execute its function body.

Generic identity in Ruddy requires no descriptors. Native extern identity needs
them for conversion. Neither operation specializes a generic function body for
each source type.

## Native JavaScript conversion

Ordinary extern arguments and results use these encodings recursively:

| Ruddy type | JavaScript representation |
| --- | --- |
| `Nat`, `Int` | Integer `number`, nonnegative for `Nat` |
| Fixed integers through 32 bits | Integer `number` in the declared range |
| `Nat64`, `Int64` | `bigint` in the declared range |
| `Real`, `String`, `Boolean` | `number`, `string`, `boolean` |
| `[T]` | Native array with recursively converted elements |
| Record or tuple | Object with named fields or numeric string keys |
| Sum | `{ tag: "Case", value: payload }` |
| `()` | Empty object; typed native results use the existing void contract and discard their value |
| `JsValue` | The original host value, without inspection |
| `Any` | The original authentic opaque package |
| Function | An adapter for the reviewed calling and completion convention |

Conversions copy array storage in both directions, including nested arrays.
Mutating a native argument cannot mutate the original persistent array; retaining
and later mutating a returned native array cannot mutate the imported snapshot.
Records are reconstructed from their declared fields. Opaque `JsValue`, `Any`,
and the established explicit mutable-cell ABI retain their identity.

Compiler-recognized persistent-array intrinsics retain their internal ABI. Their
reserved targets require the exact supported signatures. The runtime's internal
sum constructor remains accepted for compiler/platform interoperability.

Typed foreign results are checked. A malformed result raises a foreign contract
error, with the failing path. A Promise result is converted after completion
using the descriptors captured by its adapter. Existing callback effect coverage
and completion rules still apply; a descriptor supplies no effect handler.

For untrusted or unknown data, use checked decoding instead:

```ruddy
extern input: JsValue = "({ values: [1, 2, 3] })"
let result: std::Result { values: [Nat] } std::js::DecodeError =
  std::js::decode input
```

`DecodeError` contains `path`, `expected`, and `message` strings. Decoding checks
the requested structure and reconstructs Ruddy containers. Cyclic structural
data and failed JavaScript observations return errors. Unlike the typed void
contract, explicit decoding to unit checks for an empty object or `undefined`. Arbitrary functions can
be retained as `JsValue`; decoding cannot prove their types or effects. An object
that resembles an `Any` cannot fabricate an authentic package. Native conversion
does not retain a value's former Ruddy type; opaque `Any` transport does.

## Exports and unavailable information

Portable libraries may export unresolved requirements. JavaScript-visible roots
must have concrete native interfaces, including nested functions and containers.
Both checking and building reject unresolved requirements. Export a concrete
wrapper, or deliberately use `JsValue`:

```ruddy
@private let box = fn value => std::any::upcast value
let receive: JsValue -> Any = fn value => box value
```

The compiler does not default an unknown type or inspect values to guess a type.
Unknown row remainders can be supplied through inferred descriptors. Unsettled
field presences, producer-owned presence packages, and region-bearing types
cannot currently produce structural identity descriptors. Such requests are
diagnosed. Explicit cell externs retain their established aliasing contract;
boxing a cell or hiding its region in a descriptor is rejected. Pure function
types are representable; effectful function identities and unsupported native
callback contracts are rejected.

Source type printers retain existing syntax. Hover and debugger views explain
representation requirements separately.

## Termination and persistence

The compiler first constructs a finite graph of admitted semantic types. Alias
back edges are memoized, and the existing rejection of growing recursive type
applications remains in force. Callable shapes follow finite source occurrences;
instantiation copies graph interfaces, never source bodies. Closed subgraphs
remain erased rather than expanding a shared alias DAG into a tree.

Requirement solving adds pairs of parameter indices and demand ports to finite
sets. Substitution maps indices to finite sets of free parameters, and dependencies
are monotone edges. No solver step allocates new types or graph nodes, so recursive
groups reach a fixed point. Adapter synthesis memoizes type/profile pairs and
capture layouts before visiting recursive children. Descriptor equality compares
graph-node pairs coinductively. Runtime conversion uses an explicit work stack
with cyclic-path detection. There is no instance search or user-defined type code.

Artifacts carry per-arrow interfaces, descriptor graphs, projections, and
target-neutral reflection/conversion operations. Lowered globals retain their
callable contracts across erasure, so validation can compare every published
interface—including aliases and nested callables—with the convention used to
compile its initializer. It also checks descriptor references, argument
representations, bound positions, ports, and direct closure evidence arity.
As with the existing typed executable artifact, validation checks compiler IR
contracts; it is not a proof of arbitrary hand-written executable semantics.
Requirements participate in semantic fingerprints and compiler cache stamps.

The debugger's **Runtime types** tab shows value shapes, per-invocation needs,
conditional demand ports, and evaluation requirements. The existing LIR and
artifact views show their lowered descriptor operations.
