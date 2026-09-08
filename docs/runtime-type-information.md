# Runtime type information and JavaScript

Implementation is in progress. The current compiler has known callable
initialization and compatibility failures; see
[implementation status](../.scratch/inferred-type-representations/implementation.md).
This document is not a declaration that the feature is ready.

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
Lowering abstracts a generalized binding over those descriptors and captures
them in the resulting value. Higher-order arguments therefore retain their
ordinary callable convention after instantiation. Local bindings, partial
applications, and returned closures retain descriptors in the same way as other
captures. Generic identity in Ruddy requires no descriptors. Native extern
identity requires them to perform its conversions. Neither operation specializes
a generic function's body for each source type.

## Native JavaScript conversion

Ordinary extern arguments and results use these encodings recursively:

| Ruddy type | JavaScript representation |
| --- | --- |
| `Nat`, `Int` | Safe integer `number`, nonnegative for `Nat` |
| Fixed integers through 32 bits | Integer `number` in the declared range |
| `Nat64`, `Int64` | `bigint` in the declared range |
| `Real`, `String`, `Boolean` | `number`, `string`, `boolean` |
| `[T]` | Native array with recursively converted elements |
| Record or tuple | Object with named fields or numeric string keys |
| Sum | `{ tag: "Case", value: payload }` |
| `()` | Empty object; an incoming `undefined` also represents unit |
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
data and failed JavaScript observations return errors. Arbitrary functions can
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

Demand propagation adds members only to finite parameter sets of already solved
bindings. Recursive groups reach a finite fixed point. Descriptor synthesis
memoizes structural graph nodes and admitted recursive alias applications;
equality compares graph-node pairs coinductively. Runtime conversion uses an
explicit work stack with cyclic-path detection. There is no instance search,
user-defined descriptor execution, or recursive specialization.

Artifacts carry requirements, descriptor graphs, and target-neutral reflection
and conversion operations. Validation checks descriptor references, descriptor
arguments, and exported evidence layouts. Requirements participate in published
semantic interfaces and compiler cache stamps.
