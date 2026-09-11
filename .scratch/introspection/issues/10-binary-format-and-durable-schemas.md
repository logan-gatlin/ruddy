# 10 Positional binary format, canonical profiles, and dynamic registries

Status: resolved
Type: task
Blocked by: 06

Schema-directed binary reader/writer over the same protocol (no runtime type
tags), named canonical JSON profile, explicit schema/version IDs, dynamic `Any`
encoding through an explicit API and caller-selected registry.

Resolution: `std/binary.rud` runs the codec protocol positionally (no names
or tags on the wire, 64-bit integers, UTF-8 text with a length, counted
sequences, indexed cases) with `encode_versioned`/`decode_versioned` for
application-owned schema identities; `codec::Registry` with `register`,
`entry`, `encode_any`, and `decode_any` for dynamic values; and
`json::canonical`/`encode_canonical` for the named canonical profile.
