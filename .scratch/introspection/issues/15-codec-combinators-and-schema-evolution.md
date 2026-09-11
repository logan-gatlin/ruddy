# 15 Codec combinators, caching, and binary schema evolution

Status: open
Type: task
Blocked by: 06, 10

Spec section "Representation policy": "Provide ordinary projection,
validation, record, sum, rename, default, and version-alternative
combinators", and "Caching may share closed default policies". None of the
seven combinators exists, and `derive_encoder`/`derive_decoder` re-derive on
every call.

Spec: "Ordinary tree codecs reject active-path cycles" — `codec::Failure`
declares `#Cycle` and nothing constructs it.

Spec: "Skipping unknown binary data requires a wire length, wire kind, or
source schema. Schema evolution may need both a source schema and a target
decoder." `std::binary`'s `skip_value` answers `#Unsupported`, and
`decode_versioned` compares an exact header, so no source-schema path exists.

Also open from the same sections: a schema-traversal limit, a canonical
buffering limit, and a typed field lookup that "can return an optional field
witness after checking both name and requested field type".

## Comments

Implementer handoff at `cd906cf`: the shared wire vocabulary is also incomplete.
The current concrete `Read`/`Write` effects expose primitive numbers/text,
records, sequences, and variants, but no byte-string, map, tuple-specific, or
extension operations. They also cannot directly write JSON null for the
explicit nullable policy described in the spec. Complete the portable protocol
and explicit policy surface without making structural aliases acquire nominal
encoding behavior. A format may reject a representation it does not support,
but a reusable codec needs a way to request that representation.

Test explicit bytes, nullable and tuple policies separately from the default
structural representation; test one codec against two format handlers and
source-schema/version decoding independently of the exact-header gate. Do not
route custom policies through untyped foreign objects or require a common
document AST. Coordinate resource limits with
[23](23-enforce-codec-resource-limits.md) and extra effects with
[12](12-streaming-codecs.md).

The spec permits closed-policy caching; it does not require caching as a
correctness milestone. Prioritize correct traversal, limits, policy isolation,
and cycle behavior. If a cache is added, include target domains and all policy
identity/dependencies needed to avoid reusing a codec under a different policy.
