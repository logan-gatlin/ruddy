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
