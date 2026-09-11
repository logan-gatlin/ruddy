# 06 Codec protocols, derivation, and JSON

Status: open
Type: task
Blocked by: 04

Spec sections: "3. Inferred serialization and explicit codecs", "A protocol
that works beyond JSON", "Representation policy", "Numeric conversion and JSON
documents", "Streaming, failure, and limits".

- `std/codec.rud`: `Read`/`Write` effects with concrete typed operations and a
  checked session stack, `Encoder`/`Decoder`/`Codec`, `derive : Mirror 'a ->
  Result (Codec 'a) DeriveError` written in Ruddy over the views (graph-aware
  for recursive types), errors with type paths and value paths.
- `std/json.rud`: buffered JSON reader/writer handlers over the protocol,
  `encode`/`decode`/`encode_with`/`decode_with`, strict structural profile
  (records as objects, `{tag, value}` sums, unit `{}`, numeric-label tuples,
  reject unknown/duplicate/missing), exact number tokens in the document API,
  correctly rounded `Real`, integral fraction/exponent spellings for integers,
  trailing-token and incomplete-input rejection, limits.
- Retire the `ffi::decode` route for typed JSON decoding.

Seams: generated JS execution of Ruddy programs; `tests/src/stdlib_apis.rs`.
