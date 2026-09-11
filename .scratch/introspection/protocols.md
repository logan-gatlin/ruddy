# Backend-neutral introspection through explicit protocols

Status: design alternative for the introspection proposal; no implementation changes.

This design puts extensibility at three separate seams: semantic reflection, data formats, and foreign calling conventions. A shared type witness gives these modules common knowledge of Ruddy values. It does not impose a universal serialized value or a universal host representation.

All interface examples below are **proposed type-system pseudocode**, not accepted Ruddy syntax. In particular, `forall`, `exists`, indexed shape cases, and explicit effect-row quantification require language support or compiler-supported eliminators with equivalent semantics.

## Recommendation and architectural seam

Make `Type a` compiler-authenticated semantic evidence. Give library authors safe typed reflection over that evidence. Build format-independent `Encoder a` and `Decoder a` protocols as explicit ordinary values. Provide generated structural defaults from `Type a`. Give each foreign runtime its own reviewed `ForeignAdapter host a`, sharing safe structural traversal where useful but owning its ABI, resources, and callbacks.

| Module | Interface | Implementation and variation |
| --- | --- | --- |
| `reflect` | `Type a`, structural equality, shallow typed views, checked construction | Compiler and backend agree on type identity and safe value access. |
| `codec` | Separate encoders and decoders, structural derivation, schema and custom composition | Ordinary Ruddy code maps application values onto a data model. |
| Format modules | Buffered and streaming readers/writers implementing the codec protocols | JSON, CBOR, a schema-directed binary format, and application protocols choose their own representations. |
| `ffi` and host modules | Reviewed adaptation of typed calls and opaque foreign values | JavaScript, C, and a Wasm adapter choose layout, allocation, error/completion conventions, ownership, and callable wrappers. |

The compiler, reflection operations, and codecs are in-process modules. Actual foreign runtimes are external adapters. A JSON parser is an in-process format adapter, even when its input arrived over HTTP. This distinction keeps foreign ownership and effects out of the ordinary serialization interface.

Depth comes from letting one structural encoder work with several formats and one format reader work with many custom decoders. Locality comes from keeping a numeric range check in the semantic conversion module, a JSON number-token rule in JSON, and a C allocation rule in the C adapter. Two required initial format adapters—JSON and a binary format that omits type tags—make the format seam concrete. A second foreign adapter need not ship immediately, but its lowering plan must be representable without JavaScript concepts.

## What exists and what changes

`src/reification.rs` already constructs finite structural descriptor graphs and recognizes `$anyUpcast`, `$anyDowncast`, and `$ffiDecode`. `src/reification/conventions.rs` already tracks latent descriptor requirements through callable shapes. These are valuable foundations: retain inferred evidence passing, regular graphs, separate compilation, and exact structural equality.

The current implementation also couples generic native conversion to `ToJs`/`FromJs`; `src/backend/type-runtime.js` owns its record, sum, array, and callback conversion rules. Its checked decode is the data-only mode of that converter. `std/json.rud` first parses to `ForeignValue` and then calls `ffi::decode`. That reuse is convenient today but cannot define the long-term format interface.

The current exact descriptor rejects effectful arrows rather than including their effect contract. A new promise of reflection over all values must address this explicitly. It cannot simply expose the present `Node::Arrow` and claim that it describes every function.

This proposal intentionally supersedes parts of `.scratch/inferred-type-representations/spec.md`: public type witnesses, typed inspection, explicit custom codecs, and potentially restricted existential/rank-2 operations are now in scope. It retains structural identity and rejects alias-name dispatch or open instance search.

## Three different descriptions

**Semantic type:** `{name: String, count: Nat64}` says what a Ruddy value is. Its descriptor includes exact primitive distinctions, structural fields and cases, recursion, callable effects, and relevant ownership/abstraction information. It contains neither JSON rules nor offsets into native memory.

**Data schema:** an explicit codec may encode that value as a named record, a positional tuple, a map with integer field IDs, or a string. A schema includes the selected field names/IDs/order, missing-field policy, variant mapping, integer representation, and version information. Two codecs for the same structural type may have different schemas.

**ABI/layout:** a host adapter may pass the record as a JS object, a pointer to C storage, or a flattened Wasm argument sequence. Its plan includes allocation, alignment, encodings, ownership, and completion rules. Equal Ruddy types do not imply equal ABI layouts.

The Wasm Component Model provides useful precedent for separating abstract values from concrete lifting/lowering; its Canonical ABI configures the representation of the same abstract values. This is architectural precedent, not a recommendation to make WIT Ruddy's type system. [Component Model explainer](https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md), [Canonical ABI](https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md)

## Semantic reflection

```text
Type a                         // opaque, authenticated evidence
SomeType = exists a. Type a
Same a b                       // opaque equality proof, only identity can inhabit it

type_of     : a -> Type a
same_type   : Type a -> Type b -> Option (Same a b)
cast        : Same a b -> a -> b
describe    : Type a -> TypeDescription

Field record = exists field. {
  name: String,
  type: Type field,
  get: record -> field,
  replace: field -> record -> record
}

record_fields : RecordWitness record -> [Field record]
build_record  : RecordWitness record -> [NamedDynamic] -> Result record BuildError
```

`type_of` can be erased to evidence retrieval: it does not inspect the runtime payload to guess a type. A witness-only operation can obtain an inferred `Type a` without requiring an existing `a`. Generic functions that use these operations acquire hidden evidence needs just as current boxing does. Annotated helpers do not need a trait or `requires` clause, and ordinary polymorphism stays erased when no operation demands evidence.

Type descriptions are ordinary immutable data suitable for tools. They are **not evidence**: parsing or editing a description cannot manufacture `Type a`, an equality proof, or valid payload accessors. Equality uses semantic graph comparison, not a hash, source alias name, allocation address, or backend shape. A printable name is optional provenance, not identity.

Use shallow views rather than eagerly materializing a reflected tree. A record exposes field accessors, an array exposes typed iteration, and a sum exposes its active case and typed payload. A recursive descriptor can be traversed with a visited set; traversing a recursive value is a separate operation with separate depth and cycle limits.

Heterogeneous fields require an existential: every field has a particular hidden type that links its witness, getter, and replacement function. A caller may unpack it only inside a polymorphic continuation. Returning independent untyped `type` and `get` fields would lose that relationship. `build_record` is the convenient dynamic interface: it checks every supplied value's exact type, duplicates, required fields, and unknown fields before producing the result. Optimized derivation can use a compiler-authenticated typed builder instead of repeating those checks.

The full reflection surface has three capability levels:

1. Metadata inspection: available for any type the compiler can describe, including functions, cells, and abstract/package nodes. Hidden representation details remain hidden.
2. Immutable structural inspection/construction: available for values with exposed record, sum, array, or primitive structure.
3. Operations on identity-bearing values: explicit typed operations with their original effects; these are not consequences of possessing metadata.

A function view reports its input, result, and effect contract. It does not enumerate captures, claim code equality, invoke the function, or prove a foreign implementation follows that contract. A cell view can report its element type and region dependency; reading or writing its contents requires the appropriate mutation effect. An opaque resource reports its contract and identity where exposed; reflection cannot manufacture or clone ownership.

Scoped views and existential packages must preserve region dependencies. A general `Any` that hides a region-bearing value cannot become an escape hatch. Either its type preserves the dependencies, or the compiler rejects that escaping package. A scoped alternative can use a fresh region and a rank-2 callback:

```text
with_view : a -> (forall scope. View scope a -> b) -> b
```

The result `b` must not mention or capture the fresh scope. This is a static Ruddy rule, not new foreign retention enforcement. Existing hidden presences/packages remain producer-owned; reflection can describe their abstraction and inspect only what available evidence authorizes.

An exact function witness must include the semantic effect row and relevant region/effect identities, or retain those parts as opaque authenticated evidence. Reifying a function as an effect-free arrow would make exact casts unsound. If a contract cannot yet be represented safely, report an explicit unsupported operation while still allowing a restricted opaque description.

## Format-independent encoding and decoding

Serde demonstrates the useful division: a data structure chooses operations from a shared data model, while a format implements how those operations map to its output/input. The split avoids writing every type/format pair by hand. Ruddy should take that separation without importing Rust traits, nominal derive dispatch, or instance search. [Serde data model](https://serde.rs/data-model.html)

```text
Step effects a = Result a CodecError + effects

Encoder a = {
  schema: EncodeSchema,
  write: forall effects. Writer effects -> a -> Step effects ()
}

Decoder a = {
  schema: DecodeSchema,
  read: forall effects. Reader effects -> Step effects a
}

Codec a = { encoder: Encoder a, decoder: Decoder a }

derive_encoder : Type a -> Result (Encoder a) DeriveError
derive_decoder : Type a -> Result (Decoder a) DeriveError
```

The base encoder/decoder has no ambient effect except its supplied writer/reader. Configuration such as defaults and validation is captured in an explicit codec value. If a custom conversion needs other effects, expose an effect-parameterized variant whose additional effects remain visible; do not hide network access inside an apparently pure decoder.

Keep encoding and decoding independent. A decoder can accept historical field names, multiple versions, or numeric strings while the encoder emits only the current representation. An output-only projection need not pretend to have an inverse. A combined codec may document round-trip laws, but arbitrary custom code is not mechanically guaranteed to satisfy them.

The protocol's data model distinguishes unit, booleans, integer domains/widths, floating-point values, text, bytes, sequences, tuples, records with known field schemas, maps with data keys, and variants. These are *format operations*, not additional Ruddy semantic type constructors. For example, an explicit codec can expose `[Nat8]` as bytes; its structural default can remain a sequence.

The writer accepts nested callbacks so aggregate members can be emitted incrementally:

```text
Writer effects = {
  scalar: Scalar -> Step effects (),
  sequence: SizeHint -> (SequenceWriter effects -> Step effects ()) -> Step effects (),
  record: RecordSchema -> (RecordWriter effects -> Step effects ()) -> Step effects (),
  variant: VariantSchema -> CaseKey -> (Writer effects -> Step effects ()) -> Step effects (),
  ...
}

SequenceWriter effects = {
  element: (Writer effects -> Step effects ()) -> Step effects ()
}

RecordWriter effects = {
  field: FieldKey -> (Writer effects -> Step effects ()) -> Step effects ()
}
```

A JSON writer writes delimiters and tokens; a CBOR writer can use known lengths; a binary record writer can omit names under a supplied schema. The protocol documents exactly-once, in-order consumption, what an omitted field means, and whether size hints are authoritative. Incorrect custom implementations fail through the interface rather than letting invalid layouts create typed values.

Default structural derivation has predictable behavior: fields sorted by a documented structural rule, exact numeric domains preserved, explicit sum tags and payloads, and no alias-sensitive conventions. `Option a`, byte buffers, and tuple syntax do not gain magical behavior because of a declaration's name. Explicit `nullable`, `bytes`, and `tuple` codec constructors select those data-model conventions. A named alias and its underlying structural type continue to compare equal.

Schemas belong to codec values and can therefore be inspected without encoding a sample value. A custom decoder that accepts several alternatives describes a union or supplies deliberately incomplete metadata. The schema interface must represent `opaque/custom` when no accurate finite description is available; it must not pretend that a successful example proves all accepted input shapes.

## Deserialization is not encoding run backward

A reader must be driven by the expected schema. It cannot require that every format read a universal tagged `Value` first. Serde explicitly distinguishes self-describing `deserialize_any` from typed requests needed by non-self-describing formats. Depending on the former excludes formats such as Postcard. [Implementing Deserialize](https://serde.rs/impl-deserialize.html)

```text
Reader effects = {
  scalar: forall a. ScalarHint -> ScalarVisitor a -> Step effects a,
  sequence: forall a. ElementSchema -> SequenceVisitor effects a -> Step effects a,
  record: forall a. RecordSchema -> RecordVisitor effects a -> Step effects a,
  variant: forall a. VariantSchema -> VariantVisitor effects a -> Step effects a,
  ...
}

SequenceAccess effects = {
  next: forall a. Decoder a -> Step effects (Option a)
}

RecordAccess effects = {
  next_key: () -> Step effects (Option FieldKey),
  value: forall a. Decoder a -> Step effects a,
  skip: DecodeSchema -> Step effects ()
}
```

The format reader calls the visitor's appropriate case; the visitor supplies the child decoder **before** that child's bytes are consumed. `next_key` may obtain a textual key from JSON, an integer field ID from a tagged binary format, or a schema position from a positional format. A positional format gets field order and each expected type from the schema rather than discovering them in the input. A record reader may alternatively call a sequence-shaped visitor when that is the format's record representation.

`ScalarHint` constrains what the decoder expects, but the visitor still validates the actual representation. A JSON reader can present an exact number token or a checked integer representation when the target asks for `Nat64`; it must not first round through a JavaScript `Number`. Conversions that accept numeric text or narrowing are explicit decoder policy, with range checks.

An optional `self_describing` capability enables dynamic document values or format-to-format transcoders. It is not a required primitive. Transcoding a non-self-describing input needs a source schema. Skipping unknown data also needs a length, a wire kind, or the source field's schema; `skip` cannot magically determine where arbitrary unknown bytes end.

Schema evolution in a binary format can need **both** the source schema and the target decoder. Provide an explicit source-schema registry or envelope ID when the format does not carry enough information. A compiler type hash is neither a stable wire-version identifier nor a migration policy.

Decoding is allowed to be stateful through explicit closure context/seed values: shared lookup tables, interning, allocation arenas, or application configuration do not require global codec registration. Serde's `DeserializeSeed` is direct precedent for supplying such state rather than forcing a context-free type-level instance. [DeserializeSeed](https://docs.rs/serde/latest/serde/de/trait.DeserializeSeed.html)

## Streaming and allocation

Buffered helpers are ordinary wrappers:

```text
json::encode      : a -> Result String Error
json::decode      : String -> Result a Error
json::encode_with : Encoder a -> a -> Result String Error
json::decode_with : Decoder a -> String -> Result a Error

json::write_with  : Encoder a -> ByteSink effects -> a -> Step effects ()
json::read_with   : Decoder a -> ByteSource effects -> Step effects a
```

Inference gets `a` from the input for encoding and from the expected result for decoding. Convenience functions request its hidden `Type a` and derive a structural codec. Unsupported default derivation returns a clear error for functions, live cells, resources, or opaque values. An optimization may specialize/cache successful derivation; that does not change the source semantics. Custom codecs do not need reflection if they already know how to access their values.

Streaming does not mean every decoded collection is materialized. A visitor may reduce elements to a count, write selected fields to another sink, or construct an application aggregate. An explicit `collect` decoder allocates an array. Consumption scopes prevent an iterator from silently outliving its reader; buffering or ownership must be explicit when values escape.

Own strings/bytes by default. A later borrowed-view interface must tie the result to the input region and distinguish transient, borrowed, and owned chunks. Input streams may reuse buffers and cannot promise borrowed results survive the next read. Serde exposes these distinctions explicitly through visitor methods and deserializer lifetimes. [Deserializer lifetimes](https://serde.rs/lifetimes.html)

Use structured error paths with field/case/index segments, optional source offsets, and typed causes. Bound nesting, allocated bytes, collection sizes, and traversal work. Streaming output may contain a prefix when a later element fails; document that. Buffered convenience functions can discard incomplete output. Cancellation and cleanup belong to source/sink effects and their resource adapters, not to an implicit global runtime.

## Explicit customization and evolution

Provide ordinary codec combinators: `map` for encoding projections, `validate/map_result` for decoding, record/variant builders, `rename`, stable numeric field IDs, defaults, aliases accepted only on input, unknown-field policy, `omit_if`, nullable encoding, text-number encoding, bytes representation, and explicit version alternatives.

```text
current_user : Codec { name: String, visits: Nat64 }
current_user = codec.record {
  name:   field(id = 1, write_name = "name", read_aliases = ["displayName"]),
  visits: field(id = 2, codec = codec.integer_text, default = 0)
}

json::encode_with current_user.encoder value
cbor::decode_with current_user.decoder bytes
```

This is illustrative builder notation; it needs an eventual concrete syntax using structural records, selectors, and typed field witnesses. It does not require new annotation grammar.

Two equally named or structurally identical aliases can use different codec values at different call sites. No process-global registry, import-order behavior, declaration-name dispatch, orphan rule, or overlapping instance resolution selects a codec. Customization travels as an ordinary parameter or closure capture.

Do not collapse missing, null, and a present value. A field decoder sees presence separately from the value decoder; schemas state when a field may be omitted and which defaults apply. Evolution policies also state how duplicate fields, unknown tags, unknown fields, and incompatible versions are handled. A lossless round trip of unknown fields requires an explicit retention representation rather than silently dropping them.

For specialized data models—decimal numbers, timestamps, graph references, application tags—allow explicit namespaced extension schemas with versioned payload schemas. A format either implements the extension, uses a codec-supplied portable fallback, or returns unsupported. This adds capabilities without changing semantic type equality or requiring every format to support every feature. No claim is made that one common data model preserves every XML/CBOR/custom-format distinction automatically.

## Foreign adaptation and reflection over live values

```text
Foreign host                        // opaque host value; host-indexed

ForeignAdapter host a = exists wire. {
  wire_type: AbiType host wire,
  contract: ForeignContract,
  lower: HostContext host -> a -> Result wire ConversionError + HostEffects host,
  lift:  HostContext host -> wire -> Result a ConversionError + HostEffects host
}
```

This shape is explanatory, not a requirement to allocate a universal `wire` object. A compiler can lower the plan directly to registers, stack storage, Wasm values, or JS operations. The hidden wire type and ABI evidence keep raw layout information inside the adapter.

The adapter contract supplies its own representation for sums, arrays, strings, optional fields, null/unit, and numbers. It declares copy/borrow/transfer behavior, allocation/free responsibility, alignment and byte order where applicable, failure mapping, and call completion. An `Any` package may travel opaquely only where its authentic runtime package protocol exists; it is not a portable wire format. A host value cannot be relabeled as another runtime's value because both use the same Ruddy backend.

Data conversion can use the structural traversal machinery and a data-only reader/writer adapter where its representation agrees. Live functions, handles, cells, and host objects use the foreign adaptation contract directly. They do not pass through the serialization data model. A file handle's descriptor gives enough information to route an operation, not enough to serialize the operating-system state of an open file.

WIT similarly separates plain data from resources and distinguishes owned from borrowed handles; borrow duration and destruction are part of that contract. The useful lesson is that ownership is explicit information alongside shape. Ruddy need not adopt Wasm's runtime checks for every FFI. [WIT resources](https://component-model.bytecodealliance.org/design/wit.html#resources)

**Preserve the selected trust policy in `.scratch/region-mutability/ffi.md`.** Foreign implementations/callers are trusted to honor lifetime, retention, owning-thread, state identity, allocation, and declared effect invariants. This proposal does not require revocation tokens, lifetime guards, or thread checks. Ordinary Ruddy checking still cannot knowingly erase region dependencies. A particular external ABI may itself require checks; those belong to that adapter's implementation.

A callback adapter needs the semantic function type **and** a reviewed invocation contract:

```text
CallbackContract argument result effects = {
  argument_adapter,
  result_adapter,
  declared_effects,
  handler_evidence,
  completion: Sync | Async | ...,
  curry_convention,
  region_dependencies,
  trusted_retention_and_thread_contract
}
```

These fields denote compiler/adapter metadata, not forgeable records source code can use to fabricate handlers. Effect rows, hidden representation needs, and completion protocols remain distinct. `@async` says when a call completes; it does not authorize extra effects or prove lifetime safety. Evidence captured when adapting a callback survives its later completion. A sync callback cannot promise to synchronously return a computation that its declared handlers must suspend.

Reflection can inspect callable metadata. A trusted extern declaration can establish an adapter for an effectful callable. **Checking that an unknown host value is callable cannot justify converting it into an arbitrary pure or effectful Ruddy function.** Data decoding therefore still rejects functions. Dynamic invocation, if exposed, requires an explicit reviewed callable contract and returns a computation with that contract's effects; possessing a descriptor never supplies a handler.

For JS-like hosts, property observation may run getters or proxy traps. Make that activity visible in the host adapter's effects, or use an explicitly restricted trusted plain-data observation interface. Ordinary JSON decoding from a byte/string source is independent of that problem. Retaining a foreign value preserves its host identity; converting it into immutable Ruddy data takes the required snapshots/copies.

## Compiler requirements and costs

The small trusted core must provide:

1. Authenticated `Type a` synthesis and equality proofs; finite graph construction including the semantic information needed for exact identity.
2. Safe typed payload access/construction for structural nodes; backend-specific layout operations implement the same semantic interface.
3. Existential packaging and elimination for fields/cases/dynamic values, with ownership preserved. These may begin as dedicated builtins rather than unrestricted existential source syntax.
4. Rank-2 polymorphic callbacks or equivalent compiler primitives for unpacking fields and for reusable reader/writer methods. Ordinary monomorphic record fields do not express the interfaces shown above. Do not pretend this is solely a standard-library change.
5. Evidence-demand inference through higher-order arguments, partial applications, closures, aggregates, recursive groups, imports, and exports, retaining the current terminating finite-demand discipline.
6. Separate lowered operations for semantic inspection and host adaptation; artifact validation, evidence-layout versioning, and cache invalidation when inferred requirements change.

One conservative implementation path exposes typed reflection via compiler eliminators and keeps reader/writer session types abstract. A later general rank-2/existential feature can express the same interfaces in ordinary source. The alternative is to add those language features first, accepting more type-system work in exchange for a smaller permanent intrinsic surface.

Runtime descriptor dispatch and protocol calls cost more than handwritten codecs. Keep plain values untagged, descriptors shared, views lazy, and structural derivation memoized by semantic type plus explicit schema/policy. Backends may specialize known witnesses and inline protocol adapters. Optimization does not make code generation or arbitrary user code part of type checking.

## Migration and acceptance evidence

1. Split exact semantic evidence from JS conversion plans internally, preserving today's `Any` and extern behavior. Introduce neutral lowering-operation names and per-host ABI plans.
2. Add authenticated public descriptions and shallow typed reflection. Test exact equality, recursive graphs, effectful callable metadata, and region/package restrictions before generalizing the `Any` surface.
3. Add separate encoder/decoder protocols, structural defaults, and explicit customization. Implement JSON directly over tokens/bytes and a compact schema-directed binary adapter; both must run through the same codec interface.
4. Route `json::decode` to its reader plus inferred structural decoder. Preserve `json::Value`, `parse`, and `stringify` as document-oriented conveniences with documented numeric precision; they cease to define the generic decode route.
5. Expose checked incoming and outgoing host conversion with structured errors. Keep legacy `ffi::decode` as the active-host data conversion wrapper while documenting the host-specific replacement.
6. Migrate extern and callback conversion to the host adapter plans while preserving current effects, completion handling, immutability, and the selected trust policy.

Acceptance requires more than a JSON round trip: one custom codec used in both formats; an input schema with no runtime type tags; asymmetric decoding of an old schema into a current value; exact 64-bit integer handling; streaming reduction without an intermediate tree; correct missing/null distinctions; rejected default serialization of a function; typed reflection over heterogeneous fields; region escape rejection; and callback behavior with the correct declared effects and retained evidence. Compiler artifact round trips must preserve the inferred evidence convention, and a C/Wasm-style layout-plan test must contain no JavaScript-specific primitive.

The main tradeoff is scope. A real public typed introspection facility is a language feature with existential, effect, and ownership consequences. Merely adding `$ffiEncode` is smaller, but it cannot provide this interface depth or remove JavaScript as the implicit data model.
