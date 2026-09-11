# Alternative: typed mirrors with checked construction

Status: design alternative, not implemented. All interfaces and syntax below are proposed. This deliberately exceeds the earlier inferred-representations spec, which excluded public type witnesses and general existential opening.

The recommendation is a backend-neutral `reflect` module built around one authenticated `Mirror a`. A mirror describes a Ruddy type and provides typed operations for observing and constructing its supported values. Codecs consume mirrors as ordinary pure Ruddy code. Foreign adapters consume the same mirrors while separately handling the host's representation, calling convention, effects, and ownership contracts.

This design has three public concepts: **Mirror** for type-directed work, **Codec** for a particular data representation, and **foreign adapter** for a particular host contract. Describing a type, traversing its values, and building a checked result are the mirror's interface. They are not three independent descriptor systems. An `Any` is a value packaged with a mirror under the existing exact-cast contract.

## Why this differs from the current implementation

The current compiler already has much of the hidden implementation needed: finite structural descriptor graphs, inferred per-arrow evidence demands, evidence capture in closures, and portable artifact information. Its exact descriptors deliberately reject effectful arrows, unsettled presences, cells, and unknown structural types. A separate native template can admit optional fields without establishing an exact type identity. JavaScript currently owns the structural traversal and its native conversion policy. See [reification.rs](../../src/reification.rs), [type-runtime.js](../../src/backend/type-runtime.js), [ffi.rud](../../std/ffi.rud), and the [existing spec](../inferred-type-representations/spec.md).

Keep demand inference and the distinction between exact identity and an observational shape. Move format-independent reflection and checked construction into a portable semantic interface. Stop treating the JavaScript object representation as the intermediate representation for every serialization format.

Merely publishing the current descriptor graph would be a shallow interface: every caller would need to implement safe field access, construction, alias handling, cycles, unsupported types, and matching native layouts. A mirror earns depth by keeping those implementation details local while making the same behavior available to serializers, debuggers, schema tools, and FFI converters.

## The mirror interface

An inferred constructor is the ordinary entry point:

```text
-- Proposed signatures, not current Ruddy source.
reflect::mirror : () -> Mirror 'a

let config_mirror : Mirror { name: String, retries: Nat } =
  reflect::mirror ()
```

`Mirror a` is an invariant, compiler-authenticated type. A user cannot construct one from a description record, change its index, or use a deserialized schema to manufacture one. It contains no mandatory copy of a value. It preserves the region and abstraction dependencies of its index.

The operations available through a mirror are:

| Operation | Contract |
| --- | --- |
| Describe | Inspect a finite semantic graph, including primitives, record fields, sum cases, arrays, callable contracts, and opaque/abstract positions. |
| Observe | Obtain typed constituent values through compiler-generated access operations; observing immutable data is pure. |
| Construct | Build arrays, records, sums, and primitives using typed constructor operations; check any dynamically supplied structure before yielding `a`. |
| Exact cast | Compare authentic exact witnesses and recover an `Any` payload only when its static type matches exactly. |

The read-only description is useful without construction rights. A function, cell, or opaque resource can have a descriptive mirror even when it has no generic serialization or constructor operation. An abstract position can be described as abstract without falsely claiming its hidden shape is known.

The description is a graph with stable node handles within that description. Following an edge is lazy and finite. Node handles are not language type identities or wire-format identifiers. Equality ignores alias spelling, field declaration order, descriptor allocation, and bundle location wherever Ruddy's semantic types are equal.

The authenticated mirror and an exportable schema description are distinct. A schema description is ordinary data. It can be rendered, cached, serialized, or edited; doing so grants no right to cast values. A user edits a codec or supplies a mapping if they want different wire behavior.

## Typed observation without requiring general GADTs

The essential challenge is heterogeneous fields. A record containing `name: String` and `retries: Nat` cannot expose them as `[(String, Mirror 'b)]`: that signature gives both fields one homogeneous type variable. Erasing every field to `Any` works for a dynamic interface, but loses the direct typed relationship needed for generic checked construction and introduces region-packaging problems.

This alternative uses a restricted number of existential packages. Mathematical notation makes the missing feature explicit:

```text
-- Proposed abstract interfaces. `exists` does not exist in Ruddy today.
SomeField record = exists field.
  { name: String,
    type: Mirror field,
    read: record -> field,
    bind: field -> Binding record }

RecordView record =
  { fields: [SomeField record],
    build: [Binding record] -> Result record BuildError }

ArrayView array = exists element.
  { element: Mirror element,
    read: array -> [element],
    make: [element] -> array }

SomeCase sum = exists payload.
  { name: String,
    payload: Mirror payload,
    project: sum -> Option payload,
    inject: payload -> sum }
```

A `Shape a` can then be an ordinary sum whose primitive branches contain `read: a -> Bool` / `make: Bool -> a`, or the analogous operations for a particular numeric type. Its record, array, and sum branches contain the interfaces above. These typed functions remove the need to refine `a` by pattern matching on a GADT. The compiler creates the functions only where the represented structural constructor justifies them.

Opening an array package introduces one fresh abstract element type shared by its mirror and accessors. Opening a field package introduces one fresh abstract field type shared by `type`, `read`, and `bind`. A generic codec can decode a value using `field.type`, then give the result directly to `field.bind`. It never casts a value based on a field name.

Each field package is opened inside the callback that processes it. That callback returns `Binding record`, which hides the field type again. Therefore a normal homogeneous array map can collect bindings. We do not need a rank-2 callback taking arbitrary fields; the existential opening is the one required feature. A generic recursive walker still needs a checked polymorphic recursive annotation if it recursively invokes itself at child types; this must work within the compiler's admitted annotated recursion discipline, not through unrestricted polymorphic-recursion inference.

The builder checks that each binding belongs to the intended semantic record shape, that each required field appears once, and that no unknown or duplicate field appears. Bindings are authenticated typed tokens produced by the field operation; a list of untrusted `(name, Any)` pairs is not a substitute. Two independently obtained mirrors of the same structural type must interoperate; descriptor allocation identity cannot decide field compatibility. A builder returns a complete immutable value or an error. It never publishes a partially initialized record.

Arrays can be built directly from a homogeneous typed array. Sum injection is directly typed. Reflecting a sum value can return the selected case with its typed payload in another existential package. Constructor enumeration does not require inventing a sample value.

Required and conditional fields must be distinguished. The `read: record -> field` form above is only for a field known to be present. A conditional field uses an observation that can report absence, with no reference to a value of the absent field's type. Construction must satisfy the represented presence formula and package ownership. Omission, a present unit value, a present option containing `#None`, and wire-format null are separate facts.

## Actual new compiler and language work

This is not an ordinary standard-library addition. It requires:

1. An authenticated, invariant `Mirror a` and authenticated typed construction tokens. Their indexes and scopes must survive semantic analysis, artifact validation, and lowering.
2. Scoped existential introduction and elimination for ordinary types. The current `Ty::Package` and `Scheme::existential` machinery only supports presence ownership; it is not general type existential support. Initially these packages could be compiler-owned shapes with built-in pattern opening, but that remains a new typing rule, not syntax sugar over today's records.
3. Skolem escape checking for opened types. A result may not leak a fresh field type except by a permitted existential package. Free region, presence-owner, and effect dependencies remain attached to packages, bindings, and closures rather than disappearing behind the hidden type.
4. Portable observe/construct operations generated before representation erasure. A future backend lowers these to its own record, array, and sum layouts.
5. Broader descriptor capabilities: descriptive partial shapes, exact witnessed identity where justified, and constructor availability. Existing exact identity and native optional-field policies must not collapse into one permissive mode.
6. Inferred evidence capable of supplying the concrete type/row/presence information actually needed by the requested operation. It must propagate through higher-order values, captures, recursive groups, curried calls, imports, and effect-operation values as it does today.

General GADTs, source-level equality proofs, type classes, overlapping instances, higher-kinded polymorphism, explicit type application, and rank-2 field visitors are not prerequisites for this particular interface. An alternative `Type a` GADT with direct type-refining pattern matches is viable, but would add those proof/refinement obligations rather than making them disappear.

A smaller dynamic design can avoid type existentials by exposing `Any` children and a checked `build` operation. That is a legitimate first milestone, but should not be described as already providing the typed field interface above. The major tradeoff of the mirror alternative is more compiler work in exchange for typed user-written generic libraries.

## Erasure, rows, effects, and abstraction

Reflection uses the static type at its call site. It does not inspect one value to infer an erased type variable: an empty array supplies no element type, a sum value supplies only its selected case, and one optional record value supplies no general presence formula.

Ordinary polymorphic code remains erased unless its implementation requests a mirror or forwards a requirement. A generic `encode` function acquires an inferred evidence demand for its input type. A generic `decode` function acquires a demand for its result type. A wrapper inherits those demands even when it only calls another function. A user-facing annotation need not spell a new constraint language.

Open record and sum rows work when the caller supplies evidence for the instantiated remainder. If an abstract package hides the remainder, the mirror exposes the known prefix and a sealed abstract position. Full structural traversal and inferred construction cannot pretend that position is empty; they return an unsupported/abstract-shape error, or demand that the producer supply an explicit codec. Descriptive reflection remains useful.

Settled field presences use ordinary present/absent structure. Unsettled or producer-owned presences require their own scoped evidence and formula semantics. Merely observing the fields present in this value cannot become an exact type witness. A complete implementation may construct an existential package after validating an admissible presence assignment; it must return that package under its declared abstraction, not a forged concrete record type. Until that evidence rule is implemented, reject generic construction at those positions explicitly.

Effectful function descriptions include the argument, result, declared upper-bound effect row, and effect arguments; they do not report which effects a particular invocation will actually perform. An exact function identity must include every semantically relevant callable contract and cannot erase effects. Its implementation identity is not reflected. Effect aliases are transparent, and concrete effect identities follow Ruddy's existing semantic identity rule. Compiler-local numeric IDs or pretty-printed names are not substitute identities.

A descriptor is not a handler. Obtaining a callable mirror does not enable invocation under arbitrary effects or prove a foreign function honors a contract. Description may expose a closed effect row; an abstract effect row stays abstract unless properly evidenced. Extending effect metadata reflection must not accidentally merge distinct demands just because effect rows coalesce constructor applications.

## Cells, functions, resources, and region safety

Pure inspection never dereferences a cell, calls a function, or reads an external resource. It can describe their types and label their values opaque. Serializing a snapshot requires an explicit application function that reads the cell or resource with its ordinary declared effects and produces immutable data, then invokes a pure codec.

General construction cannot create an arbitrary closure, cell, or host handle from data. Callable adapters are trusted FFI operations, and resource reconstruction is an explicit application operation with relevant effects. A serializable function identifier is a user-designed reference format backed by a registry, not a reconstructed closure.

`Any` retains exact casting of the original payload. Exact cast is neither decoding nor numeric conversion nor row-width projection. Function values may be boxed only where the exact callable contract and transitive dependencies are available. Region-dependent payloads may not become an unrestricted region-free `Any`.

For this alternative's first implementation, conservatively reject unscoped existential packaging of cells, aggregates containing them, and stateful closures. Their descriptive mirrors can still be used in scopes where the full type remains visible. Supporting scoped value packages later requires keeping all free region dependencies in accepted semantics; a hypothetical `Any region` or existential region package is not silently assumed. This avoids replacing the selected caller-region design with a new region ownership model.

The existing [foreign trust policy](../region-mutability/ffi.md) is preserved. Foreign implementations and callers are trusted to respect lifetime, retention, effect, and thread invariants. This design does not require runtime revocation, callback lifetime guards, or owning-thread checks. Source-side escape checks and complete conversion contracts still apply.

## Codecs: inferred types, explicit policy

The basic codec is an ordinary value with two pure functions:

```text
-- Proposed shape, using familiar Ruddy structural typing.
type Codec 'a 'wire = {
  encode: 'a -> Result 'wire EncodeError,
  decode: 'wire -> Result 'a DecodeError,
}

json::codec  : JsonOptions -> Codec 'a String
cbor::codec  : CborOptions -> Codec 'a [Nat8]
```

These constructors internally obtain `Mirror a`; inferred evidence supplies `a` from use. Encoding is input-directed, decoding result-directed. A convenient `json::encode` or `json::decode` is a thin use of a default codec. A generic record codec handles unrecognized record structures without hand-written schemas because the mirror already supplies the structure.

```text
-- Proposed usage; the compiler infers the codec's type parameter.
let config_codec : Codec { name: String, retries: Nat } String =
  json::codec json::defaults

let encoded = config_codec.encode { name: "Ruddy", retries: 2n }
let decoded = config_codec.decode input
```

JSON and CBOR are distinct codec adapters over this reflection seam. They do not convert through JavaScript objects or a mandatory universal JSON-like tree. Each can traverse typed mirrors directly using a parser/writer implementation. A dynamic intermediate tree can be an optional convenience. Buffered and streaming interfaces can share the same mirror traversal; streaming IO is supplied through explicit effectful source/sink adapters, and buffering is not required by reflection.

Type structure does not fully specify a useful wire schema. Codecs explicitly choose sum tagging, field names/order, option/null policy, tuple convention, bytes representation, integer encodings, unknown-field behavior, duplicate-key behavior, and defaults. Reflecting aliases does not assign a UUID/date encoding automatically. An alias has no distinct structural identity merely because it has a helpful name.

Custom codecs are ordinary typed values. Mapping combinators compose `a -> b` and checked `b -> Result a error` with an existing codec. Per-field combinators compose using typed reflected fields. An application can pass a complete codec when default inference would choose the wrong representation. No process-global type-name registry, instance search, or coherent-or-overlapping-instance rules are required.

The shared scalar model distinguishes `Nat`, `Int`, fixed-width signed/unsigned integers, `Real`, `Bool`, `String`, and unit. A codec preserves the language value exactly where its format policy promises a round trip. It must reject out-of-range, lossy, or unsupported encodings rather than inherit host coercions. JSON parsing must retain numeric lexemes or equivalent exact numeric information until typed construction; parsing through a host floating-point number and later validating cannot recover lost digits. Non-finite real values require an explicit encoding policy. `[Nat8]` becoming bytes is codec policy unless the language eventually adds a distinct bytes type.

Serialization errors carry a structured path made of record-field, array-index, and sum-case segments, plus a stable error kind and expected/actual description. Byte offset, line, column, host exception information, and human messages are optional diagnostics. Missing field, duplicate field, unexpected case, range failure, unsupported type, abstract shape, cycle, depth limit, and parse failure are distinct errors. Both outgoing and incoming conversion use this error model.

A default data codec has laws: if encoding succeeds, decoding that representation with the same codec reconstructs an equivalent immutable value; any normalization or identity loss is explicitly part of codec policy. `decode(encode(x))` does not claim to preserve function identity, resource identity, graph sharing, floating-point NaN payloads, or data a codec explicitly discards.

## FFI: shared reflection, different host contracts

An FFI data converter is structurally similar to a codec but host observation can perform effects or fail while accessing the external value. Its adapter specifies those effects. Calling a JavaScript getter, observing a proxy, consulting a native foreign object, or materializing borrowed foreign memory is not automatically a pure Ruddy computation.

A backend-specific host value remains opaque: conceptually `js::Value`, `python::Value`, or another host-specific type. A portable adapter can use a runtime-indexed opaque `ForeignValue runtime` if the language gains an appropriate distinct runtime witness; otherwise separate opaque host types are the simpler model. The current unparameterized builtin can remain an alias for the existing JS host while migration proceeds. A backend-neutral design does not require inventing one interchangeable representation for every host.

There are two distinct operations:

* Checked conversion of untrusted foreign data into a typed Ruddy value, or a structured recoverable error.
* A declared extern call using a reviewed host calling convention. Its adapter marshals data, handles callbacks and completion, and trusts the declared foreign behavioral contract. Malformed values there remain foreign contract failures unless the source explicitly selected a checked conversion interface.

The pure reflection module cannot supply the latter by itself. A C-style adapter must specify layout, alignment, integer widths, ownership, string encoding, allocation and deallocation, and calling convention. A JS adapter specifies native arrays/objects, tagged values, callbacks, and completion. These are properties of the foreign adapter, not of Ruddy structural type identity.

Mutable host arrays imported as persistent Ruddy arrays are snapshots in both directions. Opaque identity-bearing foreign values retain their host identity when simply forwarded. Cell and resource adapters preserve the declared identity and ownership semantics. An authentic transported `Any` remains opaque; native data that merely resembles its package cannot fabricate a castable Ruddy payload.

Extern roots still require concrete invocation and conversion evidence for every JS-visible nested callable position, or captured evidence already supplied by an adapted callable. The descriptor is not a host-supplied argument by default. Callback currying, handler evidence, suspension, and retained conversion state remain the responsibilities of the reviewed callable adapter.

## Recursive types and value graphs

A recursive type is a finite descriptor graph with back edges. A value inhabiting it may be a finite tree, a DAG with shared immutable substructure, or a cyclic foreign graph. These are separate cases.

Descriptor derivation memoizes graph nodes before following edges. Type comparison memoizes node pairs. It never repeatedly unfolds recursive aliases into an infinite tree or recursively specializes a generic function for every unfolding.

The ordinary data codec traverses finite immutable data with an explicit work stack or equivalent bounded-stack implementation. A repeated sibling reference can be copied; an active-path back edge produces a cycle error. Limits on depth, elements, and bytes yield explicit resource-limit errors. A graph format preserving sharing is a separate codec policy using reference IDs and a specified reconstruction protocol. Recursive type support alone does not promise identity-preserving graph serialization.

Checked immutable construction does not expose cyclic half-built objects. Importing arbitrary cycles needs an explicit graph/resource representation or a future ownership-aware constructor contract. It is reasonable for the default codec and checked FFI data converter to reject cycles while opaque foreign-value forwarding retains them untouched.

## Implementation locality and migration

Compiler type evidence is an in-process dependency. Keep its generation and authenticity compiler-owned; do not add a public replaceable "type provider" that permits forged witnesses. The mirror interface is the seam used by generic Ruddy programs and their tests. Its implementation contains descriptor normalization, typed field operations, primitive checks, builders, and graph traversal support.

Foreign runtimes are true external dependencies. Their adapters are replaceable at the FFI seam and tested against their actual host contracts. Codec algorithms are pure in-process code with format-specific parsing/writing; their IO adapters are independently replaceable. A mock compiler descriptor is unnecessary for ordinary codec tests.

An incremental implementation can proceed as follows:

1. Define backend-neutral semantic descriptors and inspect/build behavior for immutable primitives, arrays, records, and sums. Preserve today's hidden demand analysis and exact-cast behavior.
2. Implement authenticated mirrors plus scoped type-existential opening, with compile-time tests for incorrect field reuse, skolem escape, and hidden region escape. If that language work is deferred, explicitly ship the dynamic `Any`-child subset instead.
3. Build JSON encode/decode and a materially different binary codec over the same mirror interface. This makes the format seam real and demonstrates that it does not depend on JavaScript numeric/object conventions.
4. Reimplement checked JS data conversion over the mirror operations, preserving existing extern calling conventions and foreign contract failures. Move host observations into the explicit host adapter.
5. Extend descriptive reflection to effectful callable contracts, abstract presences, cells, and resources; add construction capabilities only where their invariants are fully specified. Treat portability of numeric semantics as a language contract to settle rather than adopting another backend's defaults accidentally.

Portable artifacts publish unresolved mirror needs and reviewed observe/construct operations. Schema/version/cache stamps change when evidence conventions or semantic shapes change. Import validation checks descriptor references, package scopes, field ownership, constructor indexes, and evidence layouts. Optimization can fuse generic traversal, specialize known mirrors, and eliminate temporary bindings without changing the interface.

Tests cross the same source-to-bundle, artifact/import, and generated-host execution seams as existing reification tests. Add codec round trips including empty arrays, recursive data, exact wide integers, absent fields, and error paths. Add negative compilation tests for forged/misused witnesses and region escapes. Verify no new format requires a compiler case solely to name the format. Rust tests run only through `just test`.

## Borrowed ideas and limits

Haskell's `Type.Reflection` uses a type-indexed `TypeRep`, an existential `SomeTypeRep`, and equality evidence returned by `eqTypeRep`. Its documentation explicitly distinguishes typed representation from the equality proof needed to recover a type. That is prior art for authentic type-indexed mirrors and checked casts; this proposal does not adopt Haskell's nominal constructor identity or its complete proof interface. [Type.Reflection documentation](https://hackage-content.haskell.org/package/base-4.19.2.0/docs/Type-Reflection.html)

Haskell's `Data.Data` separately provides generic observation and construction through `gfoldl` and `gunfold`, whose interfaces use quantified callbacks. This illustrates why type identity alone is insufficient for generic serializers, and why an apparently small generic fold can hide substantial type-system requirements. The existential-field design above is an alternative to imposing that rank-2 interface on Ruddy. [Data.Data documentation](https://hackage-content-origin.haskell.org/package/base-4.22.0.0/docs/Data-Data.html)

The paper *A reflection on types* develops safe typed reflection and the relationship between explicit runtime representations and implicit type evidence. Ruddy's inferred demand passing offers a compatible implementation starting point, but typed structural record construction, effect rows, and region dependencies remain Ruddy-specific design work. [Primary paper](https://www.seas.upenn.edu/~sweirich/papers/wadlerfest2016.pdf)

The main advantage is a deep portable reflection module that lets users write genuinely new type-directed libraries. The main cost is a small but substantive language extension for typed existential observation and its sound interaction with regions. A design claiming the same typed interface while requiring only a new JavaScript extern is omitting that cost.
