# Introspection through derived structural operations

Status: design alternative, not an implementation plan approved by the user.

This alternative puts the largest amount of behavior behind the smallest ordinary caller interface. Ruddy callers write `json::encode value` and `json::decode text`. A format author writes an ordinary recursive program over a checked dynamic view. The compiler derives the operations that connect those views to real Ruddy values. It does not know JSON, CBOR, a JavaScript object convention, or application wire schemas.

The design deliberately avoids requiring public dependent type elimination, rank-2 functions, nominal type classes, or compile-time execution of arbitrary library code. Those features might eventually be worthwhile for other purposes. They are not prerequisites for this design.

## What exists and what changes

The current implementation already has two valuable pieces:

- A finite structural descriptor graph, hidden evidence inference, and per-arrow callable interfaces in `src/reification.rs`, `src/reification/conventions.rs`, and `src/reification/interface.rs`.
- Reviewed foreign conversion and callable plans, distinct from written types, in `src/externs.rs`.

Its current descriptor is too narrow to become the universal introspection interface unchanged. `Descriptor::from_graph_policy` rejects effectful callable contracts and unsettled presences; `Node::Arrow` only contains argument and result edges. `NativeTemplate` then overlays optional field information for foreign conversion. The implementation's `Direction::{ToJs,FromJs}` and `src/backend/type-runtime.js` specialize conversion to JavaScript. `std/json.rud` parses to `ForeignValue` and invokes `$ffiDecode`, losing a clean seam between parsing, data interpretation, and host adaptation.

Retain finite graphs, hidden demand inference, regular recursion, exact `Any` semantics, and separate compilation. Replace the notion that one descriptor also defines native conversion with three independently reviewed artifacts:

| Artifact | What it means | What it must not imply |
| --- | --- | --- |
| Semantic type graph | Exact Ruddy type identity and a readable description | Access to memory, serializability, ABI, or a wire schema |
| Structural operations dictionary | Executable, checked ways to inspect and construct values of a semantic type | A default external representation |
| Foreign adapter plan | A declared calling convention, completion protocol, storage convention, ownership contract, and conversion operations for one foreign environment | Cross-platform identity or a serialization format |

The shared abstraction is a deep structural-operations module. Format modules and foreign adapters consume it at separate seams. Changes to persistent arrays remain local to backend operations. Changes to JSON option encoding remain local to the JSON module or an explicit codec. Neither change alters exact type identity.

## Ordinary usage

All following source is proposed interface spelling, not code that currently compiles. Function application, records, sums, inference, and ordinary type aliases follow existing Ruddy conventions.

```ruddy
let payload = { name: "Ruddy", retries: 3n, enabled: true }
let text = json::encode payload

let read_config:
  String -> Result { name: String, retries: Nat, enabled: Bool } json::Error =
  json::decode

let bytes = cbor::encode payload

let describe = reflect::describe payload
```

The input determines encoding's type. The expected result determines decoding's type. A generic helper forwards the hidden structural-operation requirement just as the existing compiler forwards runtime type information. There is no source `derives`, explicit type argument, or constraint clause for ordinary use.

Keep inferred-type conversion and format syntax separately useful:

```text
json::parse       : String -> Result json::Value json::Error
json::stringify   : json::Value -> Result String json::Error
json::to_value    : 'a -> Result json::Value codec::Error
json::from_value  : json::Value -> Result 'a codec::Error
json::encode      : 'a -> Result String json::Error
json::decode      : String -> Result 'a json::Error

cbor::encode      : 'a -> Result [Nat8] cbor::Error
cbor::decode      : [Nat8] -> Result 'a cbor::Error
```

JSON's parser must preserve enough numeric information to decode `Nat64` and `Int64` exactly. The existing `#Number Real` JSON value cannot represent every integer token without loss. A revised JSON value should retain an exact decimal coefficient/exponent or validated numeric token, with explicit conversions to `Real`. Parsing a JSON number through a JavaScript number first cannot be the implementation of lossless typed integer decoding.

## The concrete primitive basis

Use compiler-derived operations plus checked dynamic packages. Do not implement a user-visible generic cast from a description and arbitrary bytes.

There are three opaque runtime handles:

1. `reflect::Type`: authentic semantic type identity. It is inspectable but cannot be forged from an arbitrary graph.
2. `data::Schema`: an authentic type plus the capability to inspect and construct its immutable data values. Its reachable graph contains only supported data constructors.
3. `data::Value`: a package containing a real Ruddy payload and its authentic `data::Schema`. Internally this is existential; source programs do not unpack a fresh type variable.

An ordinary returned dictionary ties the inferred source type to this dynamic layer:

```text
type Operations 'a = {
  schema: data::Schema,
  pack: 'a -> data::Value,
  unpack: data::Value -> Option 'a,
}

data::operations : () -> Operations 'a
```

`data::operations` is a compiler intrinsic with an inferred data-operation demand. `Operations` is an ordinary structural record, not a class instance. `pack` uses the static type at that position. `unpack` performs exact semantic type equality and returns the original typed payload; it performs no coercion. Library authors may construct other ordinary dictionaries, but cannot manufacture an authentic schema or bypass checked construction.

The source-visible eliminator is untyped with respect to child type variables, but every child remains a checked package:

```text
data::view      : data::Value -> data::View
data::shape     : data::Schema -> data::Shape
data::construct : data::Schema -> data::View -> Result data::Value data::Error

data::schema    : data::Value -> data::Schema
data::type     : data::Schema -> reflect::Type
```

`View` has one case per data constructor: unit, each numeric primitive, boolean, string, array of `data::Value`, record of named `data::Value`, and tagged sum payload containing a `data::Value`. Arrays may expose a length and checked element accessor to avoid eagerly packaging all children. The choice is a performance detail of this interface, not a different semantic model.

`Shape` has corresponding cases with child `Schema` handles. Recursive children refer back to existing handles. Field and variant order is canonical by structural label, not source declaration order. Tuples are reflected according to their actual structural semantics; if Ruddy tuples are records with numeric labels, that is what reflection says. A wire codec may map those fields to an array explicitly.

`construct schema view` validates exactly the outer constructor, primitive category and value range, required fields, absence of unexpected or duplicate fields, selected variant, and exact schema identity of child packages. It then calls compiler-generated constructors to make an actual Ruddy value. It does not reinterpret a JavaScript object as a record or expose an offset write. A successful result is an authentic package for the requested schema.

It is safe for a library to choose a wrong field or construct the wrong primitive: it receives an error. It is not necessary to trust the library to prove a type equality. The only operations that establish descriptor/payload correspondence are compiler-generated packaging and checked construction. Hashes can accelerate equality, but a hash match never creates a proof by itself.

Primitive `View` variants carry real Ruddy primitive values, not platform numbers. They retain `Nat`, `Int`, every fixed integer width and signedness, and `Real`. This avoids forcing an exact integer through a lossy intermediate representation. A serializer may deliberately choose a less expressive wire format and report a representation error.

The initial data universe contains immutable finite values made from these primitives, arrays, records, unit and tagged sums. It excludes functions, cells, foreign objects, and unexamined `Any`. Such types remain describable and may have foreign adapters. Exclusion from automatic data operations is a capability decision, not an absence of type identity.

## A user-written format is ordinary Ruddy code

The public format seam is a structural record. A format can produce text, bytes, an AST, a database parameter array, or another representation:

```text
type Format 'wire = {
  write: data::Value -> Result 'wire codec::Error,
  read: data::Schema -> 'wire -> Result data::Value codec::Error,
}

codec::encode : Format 'wire -> 'a -> Result 'wire codec::Error
codec::decode : Format 'wire -> 'wire -> Result 'a codec::Error
```

The generic encode implementation obtains `data::operations ()`, packages the input, and invokes `format.write`. Decode obtains the same dictionary from its expected result type, invokes `format.read operations.schema`, then checks `operations.unpack`. An incorrectly implemented format returning a valid package of the wrong type becomes an explicit codec error, never a cast.

A JSON writer recursively matches `data::view`. Its record case writes object keys and recursively writes child packages. A JSON reader first matches `data::shape expected`; its record case requires the correct member names, recursively reads each child against its field schema, and calls `data::construct expected (#Record children)`. Arrays and sums use the same pattern. A numeric case parses the token into the requested Ruddy numeric kind before checked construction. This is sufficient for third-party generic codecs without compiler changes.

`data::Value` is a one-layer executable view of a typed value, not a second eagerly copied universal AST. A writer can stream traversal using ordinary callbacks or effects without first allocating a whole value tree. A decoder may keep a frame stack and construct completed children bottom-up. No universal streaming protocol needs to be built into semantic type identity. Streaming format interfaces can add source/sink effects at the format seam; the buffered interface above remains the ordinary convenience layer.

The compiler may specialize the generated operations and fuse view/build calls when profitable. An initial implementation can use dictionaries and small packages throughout. Performance optimization must preserve the same checked format interface and diagnostics. There is no need to make whole-program specialization the language's compilation model.

## Reflection beyond serialization

`reflect::describe value` exposes a readable description of the value's static source type, not an inferred type based on its contents. Type descriptions expose primitive identity; arrays; record and sum labels; recursion; open-row structure when abstract; arrows including effects; region binders; and abstract or existential packages. A debugger may show an abstract package without exposing its hidden witness.

Use a separate exact-identity seam:

```text
reflect::type_of : 'a -> reflect::Type
reflect::equal   : reflect::Type -> reflect::Type -> Bool
reflect::describe_type : reflect::Type -> reflect::Description
any::type_of    : Any -> reflect::Type
any::downcast   : Any -> Option 'a
```

An expected-type-only request can use an ordinary dictionary analogous to `Operations 'a`, with `pack: 'a -> Any` and `unpack: Any -> Option 'a`. Its function fields make `'a` inferable without adding a public `Type 'a` GADT. Compiler-generated identity evidence may be omitted if unused.

For reflective inspection of arbitrary boxed values, `reflect::view : Any -> reflect::View` exposes public structural children as `Any`, with special opaque cases for functions, cells and foreign resources. It must never run a closure, read a cell, invoke a property getter, or infer a function contract from a host callable. A function view can show argument, result and effect metadata without granting an invocation operation.

This requires `Any` packages to retain access to the appropriate structural operation table, in addition to exact type metadata and payload. Generate these tables lazily or share them with demanded dictionaries. This cost belongs to reflection use, not every ordinary value. `data::from_any : Any -> Result data::Value data::Error` checks the boxed type's data capability at runtime. This is useful for dynamic inspectors; the ordinary typed encode path obtains the capability statically and avoids this failure mode.

General reflection may reconstruct immutable public products and sums using checked child packages. It must reject construction of functions, mutable cells, foreign handles, or hidden abstract representations. A separate mutation interface must carry the actual mutation effect; a pure reflection operation cannot silently read or write state.

### Exact identity, effects, regions and abstraction

Semantic arrow descriptions include argument, result, effect row, and relevant quantified dependencies. Evidence calling conventions remain a separate adapter concern: two extensionally identical semantic function types do not become different types merely because one implementation needs extra hidden dictionaries. Boxing first normalizes or captures the hidden callable convention, as existing `Shape::Sealed` intends.

Effect labels use the identity rules already assigned by Ruddy's semantic type system. If a declaration has nominal effect identity, its portable identity needs a stable declaration key. That does not make structural user type aliases nominal. Bound type, row, presence and region variables use alpha-equivalent binders rather than spelling, compiler-local symbols, or allocation addresses. Scoped free region identities can only be compared while their ownership remains valid.

Exact `Any` downcasts compare the complete semantic contract. `Nat8` is not `Nat`; a narrower sum is not the same sum; a record with an extra field is not the same record; an effectful function is not a pure one; and a function with different effect requirements must not cast successfully. A successful downcast returns the original payload rather than running a codec.

Ordinary unrestricted `Any` must continue to reject boxing a value whose hidden region dependencies would escape. In particular, adding richer metadata is not permission to box a region-dependent cell or a closure capturing it into a region-free package. Exact `Type` handles mentioning free local region identities require the same scope discipline: allowing the identity handle alone to escape would expose a fresh region token and can break isolation even without exposing the cell. An unrestricted textual description can instead show a deliberately abstract region placeholder; it is not an exact identity witness. For a complete extension to scoped value reflection, add compiler-tracked scope dependencies to reflective packages and exact type handles, including dependencies hidden in closures and aggregates. This can be an inferred dependency row on a dedicated scoped package, but it is a real semantic feature and must be designed and checked across generalization, storage and return positions. A cosmetic `'r` parameter that loses multiple captured regions is insufficient.

The recommended initial implementation supports full type descriptions of well-scoped callable contracts, opaque views of safely boxable functions, and explicit rejection of region-erasing packaging. Region-owned value inspection can use ordinary typed operations under the existing mutation effect until scoped reflective packages exist. This is an explicit capability limit, not a claim that region metadata alone makes existential packaging safe.

## Rows, presences and recursive types

Open row polymorphism composes operations for the known fields with an inferred dictionary for the remainder row. At a concrete call that remainder describes the actual statically instantiated fields. Do not infer missing row information by observing the input object or the first array element. Thus an empty generic array works whenever the caller supplies its element operations; a decode with an entirely unconstrained target remains ambiguous.

Known present fields must be constructed, known absent fields must not be. Presences settled during inference produce ordinary data schemas. An abstract presence variable is not independent JSON optionality: correlations between fields and producer-owned presence packages must be preserved. For example, one hidden presence governing two fields cannot be decoded by independently accepting either field.

The initial automatic data derivation rejects unresolved conditional presences and producer-owned existential packages when it cannot construct the exact ownership-preserving value. Reflection still describes their abstraction. A generic program can acquire data operations once its caller supplies a settled instance. A future package-specific adapter may expose an explicitly public wire sum and use a checked introduction operation respecting its package rules. It must not flatten an existential package into an ordinary record or publish its hidden type identity by accident.

Recursive types use finite graph nodes and back edges. Allocate dictionary slots for a recursive group before filling their operations; recursive calls use the completed slots lazily. Do not recursively expand aliases into infinite dictionaries. Reject non-regular type expansion under the language's existing recursion restrictions.

Recursive types and cyclic runtime values are different. A tree codec accepts arbitrarily nested finite trees subject to explicit resource limits. Cyclic foreign graphs require a graph codec with declared object IDs/reference rules; ordinary data decoding rejects cycles. Structural data serialization does not promise preservation of shared reference identity. Conversion uses work stacks, node/depth/byte limits, and active-path cycle detection where host values can be cyclic.

## Hidden evidence and termination

Extend the existing per-arrow requirements from a bare parameter ID to a finite capability key and parameter ID. The compiler-owned keys are exact identity, structural view/build support, immutable-data support, and reviewed foreign-adapter support for a fixed adapter family. These are not user-definable instances, and no arbitrary search runs during inference.

For supported structural constructors, dictionary synthesis follows fixed rules: primitive constants, arrays from element operations, products/sums from child operations, and guarded recursive references. A data demand recursively demands data support for every reachable child. Known unsupported constructors fail at compile time. Generic unknowns produce hidden obligations; arbitrary `Any` content is checked at runtime only through the explicitly dynamic interface.

Preserve the current finite demand solver discipline: requirements range over a finite set of type/row parameters, capability kinds, and callable demand ports. Substitution maps demands to free-parameter demands, not endlessly expanding type expressions. Solve mutually recursive groups monotonically to a fixed point. Memoize structural dictionary construction by regular graph nodes and capability key. Keep restrictions on polymorphic recursion and prohibit compile-time execution or instance search.

Track demands at the arrow where evidence is needed. A function that creates a serializer closure may capture its dictionary; an unresolved requirement on a returned function remains visible at that later arrow. Higher-order parameters quantify over their own finite demand ports. Branches and arrays containing implementations with different dictionary needs use reviewed callable adapters. Instantiation may merge demands; canonical slot remapping must handle that without confusing independent parameters.

Dictionary acquisition is pure evidence plumbing. It must not introduce a computational effect or change ordinary let-generalization. An adapter that reads foreign getters or calls host code is effectful for separate reasons, reflected in its declared interface. Imported library artifacts publish solved demand interfaces and type/operation templates; callers are invalidated when inferred requirements change even if the written annotation does not.

At a host root, all evidence must be fixed by a concrete exported contract, captured by an already adapted callable, or provided through an explicit host protocol for dynamic values. Portable generic libraries retain unresolved requirements. A JavaScript or C caller cannot make them disappear by omitting hidden arguments.

## FFI conversion is a different adapter

Dynamic object environments can use the same data operations to copy between a host value and a Ruddy value, but the host value is not the portable data model. Conceptually:

```text
js::decode : js::Value -> Result 'a js::DecodeError
js::encode : 'a -> Result js::Value js::EncodeError
```

Those names make the environment explicit. A compatibility `ffi::decode` may select the active backend's documented host profile. A future Python backend can define Python objects, a Wasm component adapter can define its canonical ABI, and a C adapter can require a reviewed layout and allocation convention. The shared compiler does not pretend these are the same external objects or support the same types.

Separate host data conversion from typed callable adaptation. A C ABI cannot be inferred safely from a structural record alone: alignment, field representation, calling convention, string/array ownership, callback lifetime, and error/completion protocol need a declared profile or explicit adapter. JavaScript also needs choices about null/undefined, big integers, symbols, getters, tagged sums, currying and promises. None belongs in semantic type equality or JSON's codec.

Ordinary typed extern calls keep the reviewed trusted-contract model. A malformed result violates the foreign contract; a checked dynamic decode reports a structured recoverable conversion error. A foreign function's being callable never proves arbitrary Ruddy arguments, results, effects or parametricity. Importing a function at such a type requires an explicit trusted declaration and the relevant callable adapter plan.

Preserve the selected foreign trust policy in `.scratch/region-mutability/ffi.md`: foreign code is trusted to respect lifetime, retention, threading, state identity and declared effects. This design adds no mandatory revocation tokens, callback lifetime checks or owning-thread guards. Source type/effect checking must still reject known region escapes and preserve dependencies exposed by Ruddy values. The type descriptor supplies no proof about arbitrary host behavior.

Structural adapters snapshot mutable host array/object data into Ruddy immutable values, preserving persistent storage invariants. Opaque host handles retain host identity by their explicit adapter contract. There is no generic promise to clone every host object or serialize resources. Observation of host properties can execute code; an adapter must account for this in its effect and failure convention or accept only a documented inert representation.

## Wire policy and custom codecs

The compiler describes structure. A codec chooses meaning in a representation. Default mappings must be published and versioned by each format module:

- Integer widths and exact ranges; floating-point non-finite values and negative zero.
- Unit, empty records and tuples; records, field ordering and duplicate names.
- Sum tagging and payload layout; no nominal exception for an alias called `Option`.
- Unknown and missing fields; defaults, version evolution and migrations.
- Byte arrays, dates, identifiers and resources: none is inferred from an arbitrary alias name.

A conservative JSON profile uses explicit tagged sum envelopes, rejects unsupported floating values, writes integer tokens exactly, and decodes them without a float intermediate. More familiar external conventions use explicit codecs. For an alias `Option 'a = #Some 'a | #None`, automatic alias-name dispatch would break Ruddy's structural semantics. Even shape-based option detection can be surprising; the base profile should serialize its sum structure. Use an explicit option-as-null codec when that representation is desired.

Custom codecs are ordinary values:

```text
type Codec 'a 'wire = {
  encode: 'a -> Result 'wire codec::Error,
  decode: 'wire -> Result 'a codec::Error,
}

codec::via :
  { write: 'model -> 'wire_model,
    read: 'wire_model -> Result 'model codec::Error } ->
  Codec 'wire_model 'representation ->
  Codec 'model 'representation
```

This lets an application select a record-shaped wire model, rename fields, encode timestamps as strings, enforce business validation, or evolve a protocol while retaining its internal model. The transform and chosen codec travel together as explicit data. Two aliases of `String` can use different codecs at different call sites without a nominal instance system. Nested exceptional fields can be expressed in a typed whole-record projection, or through explicit typed field combinators if the library later adds them. No fragile search by field name or alias name is required.

`json::encode_with` and `json::decode_with` consume a selected codec directly. Keeping a codec in a configuration record provides locality for all callers using a protocol. A generic codec constructor derives operations once and closes over them, so repeated encoding need not redo dictionary setup.

The default format mapping is a versioned library contract, not an accidental snapshot of compiler layout. Long-lived schemas require explicit protocol names/versions, stable field/variant IDs where the format uses them, and explicit migrations. Renaming a field changes the structural type; renaming a type alias does not. Descriptor node numbers and graph allocation order must never become wire field IDs.

## Laws and failure placement

The compiler-owned laws are:

1. `unpack (pack x)` succeeds with the original payload at the same exact type.
2. Viewing then reconstructing under the same data schema succeeds and preserves the data value.
3. Successful construction yields only a value satisfying the requested semantic type and ownership rules.
4. Equivalent structural aliases share exact identity; independent backend layouts do not affect this identity.

For a codec, successful `decode (encode x)` should preserve the codec's documented value equivalence. The format must explicitly define exceptions such as normalization, floating-point NaNs, negative zero, canonical field order, or deliberately lossy projections. A lossy codec cannot advertise the lossless law. The reverse `encode (decode bytes) == bytes` is generally not required: whitespace, alternate numeric spellings and field order may normalize. Canonical formats can define a separate canonicalization law.

`Any` has a different law from a codec: a cast preserves the original package's exact static type. Reading a serialized value cannot reconstruct historical Ruddy type provenance unless an explicit envelope and trusted schema registry define that protocol. A type hash in untrusted input is only a lookup hint, never authorization to cast memory.

| Situation | Failure placement |
| --- | --- |
| Typed automatic encode of a function, cell, or unsupported existential package | Compile-time data-capability diagnostic at the demanding call |
| Unresolved generic demand in a portable library | Retained hidden requirement |
| Unresolved demand at a concrete host export | Compile-time host adapter diagnostic |
| Dynamic `Any` contains a non-data type | Runtime `data::from_any` error |
| Malformed text/bytes, missing field, bad tag, numeric overflow | Runtime codec error with structured path |
| Format cannot represent a valid value, such as a non-finite `Real` | Runtime representation error |
| Runtime-selected codec policy rejects a schema | Codec preparation error, before traversing a value where possible |
| Host contract produces invalid data through trusted typed extern | Foreign contract failure |
| Host cycle or resource limit | Runtime conversion/codec error |

Errors should carry structured path segments such as `#Field "name"`, `#Index 3n`, or `#Variant "Some"`; a formatted JSONPath-like string is a presentation choice. Include the expected semantic shape, encountered category, and a stable error code. Schema preparation can reject a format-incompatible type early, but static language type validity and runtime format policy remain different checks.

## Metadata and separate compilation

Semantic identity, human metadata, wire schema identity and operation implementation identity must be distinct. Type aliases, documentation, source spans and declaration order are useful optional reflection metadata, but not structural type identity. Two aliases may have the same semantic graph and different declaration descriptions; users explicitly request declaration metadata when that distinction matters.

Canonical exact identity includes only semantic facts: primitive distinctions, structural labels and edges, admitted recursion equivalence, effect identities and rows, package/binder structure, and region dependencies. Equality should reuse or faithfully implement Ruddy's semantic equivalence, including alpha-equivalence and recursive graph comparison. The compiler must specify its recursive equality rather than assuming two arbitrary graph hashes agree. Hashes are cached accelerators, versioned if persisted.

Portable artifacts carry validated type graphs, operation templates, capability requirements and reviewed callable interfaces. Backend lowering turns templates into layout-specific instructions and callable thunks. Artifacts do not carry executable JavaScript as the meaning of a type. Validate graph references, binder scopes, constructor arity, capability claims and hidden argument layouts before trusting imported artifacts. Bump the artifact schema and cache stamp when these contracts change.

Separate compilation can share generic dictionaries. A concrete instantiation supplies its child operations; a closed root can become a constant dictionary. Recursive groups allocate shared slots and tie them after validation. Compiler caches invalidate on inferred demand, operation schema and callable-interface changes, even when public source text does not change.

## Required language work and tradeoffs

Required compiler work:

- Expand semantic reification to faithfully describe the admitted type system, rather than treating native conversion's supported subset as the whole type universe.
- Introduce authentic `Type`, `Schema` and `data::Value` runtime handles and recognized operations. These may be compiler-backed standard-library types, but they are new trusted primitives.
- Derive checked construction, structural views and data-capability dictionaries; normalize callable evidence when packaging functions.
- Extend hidden demand inference and artifact interfaces with finite capability kinds and verified operation templates.
- Keep region dependencies through any accepted package operation, or reject unsupported region-erasing requests. General scoped dynamic reflection requires additional scope-tracking semantics.
- Lower structural operations per backend independently from foreign layouts and wire formats.

Not required: nominal classes, user instance search, public equality proofs, arbitrary `Type -> 'a` casts, GADTs, rank-2 callbacks, dependent fields, macro execution, monomorphization, or tagging every ordinary value.

The main cost is dynamic package allocation and checked reconstruction for generic libraries. One-layer views and generated specialized operations limit it; later optimization can fuse traversals. The main expressiveness limit is that a library cannot use a dynamically discovered field type as a new static type variable. It can inspect, validate, transform and rebuild through checked packages. This is sufficient for format codecs, validation tools, tree editors, inspectors, schema exporters and FFI data adapters. Type-preserving generic algorithms needing static field-type refinements would justify a separate typed-reflection language extension later.

This is deeper for ordinary callers than exposing the whole type theory as their first serialization interface. It also has a clean implementation seam: without the structural-operations module, each format and each backend would need to recreate inspection, construction, recursion and safety checks. Those responsibilities are real leverage, not a pass-through wrapper.

## Relevant primary comparisons

Scala 3's compiler supplies `Mirror` for product/sum structure, while library derivation methods create operations such as equality. Its documentation also shows that recursive dictionaries need lazy initialization. The useful lesson is to keep shape generation small and let libraries own behavior; Ruddy should not inherit Scala's type-member, inline and implicit-search machinery merely to get inferred codecs. [Scala 3 type class derivation](https://docs.scala-lang.org/scala3/reference/contextual/derivation.html)

GHC's `Generic` represents data using units, constants, metadata, sums and products, with `from`/`to` operations connecting representation and source values. That is precedent for deriving safe structural operations independently of any serialization format. Ruddy's dynamic checked packages are a different choice because the current source language does not expose GHC's associated representation types and generic class machinery. [GHC generic programming](https://ghc.gitlab.haskell.org/ghc/doc/users_guide/exts/generics.html)

System.Text.Json separates generated metadata from generated serialization fast paths and allows applications to disable reflection defaults. The relevant lesson is that a metadata interpreter and optimized generated operations can coexist behind one caller interface, and an ahead-of-time backend need not depend on runtime host reflection. The proposed Ruddy mechanism is language-wide rather than JSON-specific. [System.Text.Json source generation](https://learn.microsoft.com/en-us/dotnet/standard/serialization/system-text-json/source-generation)

## Migration and verification

1. Specify exact semantic graph identity and the compiler-owned operation/capability algebra. Preserve current source-level `Any` behavior and callable evidence while extending descriptions.
2. Implement data operation derivation, checked `data::Value` construction and the public format seam. Implement both a text codec and a binary codec against it to establish that the format seam genuinely varies.
3. Rewrite typed JSON encode/decode through JSON parsing and the data seam. Preserve numeric tokens exactly. Keep `parse`/`stringify` convenience separately useful.
4. Migrate JavaScript data conversion to its explicit host adapter profile. Keep reviewed function, cell, completion and effect plans in the foreign module. Retain a compatibility `ffi::decode` forwarding interface during migration.
5. Add full arbitrary-safe `Any` inspection and dynamic data capability discovery. Expand scoped packaging only after its source dependency semantics are specified and tested.
6. Produce at least one non-JavaScript lowering or independently executable backend-neutral operation interpreter. Two format adapters establish format independence; only a second lowering establishes that implementation has avoided a JavaScript layout dependency.

Tests should cross the compiler/library interfaces: source-to-artifact and source-to-execution for inference and dictionaries, ordinary format round trips and errors for codecs, and host-facing behavior for foreign adapters. Include independent aliases, recursive schemas, empty generic arrays, integer extrema, conditional presence rejection, higher-order demand ports, partial application, returned closures, effectful function identity, region-escape rejection, imported dictionaries, malformed artifact rejection, and dependency invalidation when only inferred demands change.

Use generated data constrained to the lossless codec subset for round-trip properties, plus explicit cases where losslessness is unavailable. Cross-format and cross-backend golden fixtures should validate external contracts, not private descriptor node IDs. Run Rust tests only through `just test`, as required by this repository.
