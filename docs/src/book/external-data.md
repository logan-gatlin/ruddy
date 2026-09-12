---
doc: true
bookNavigation:
  previous:
    path: "/book/io.html"
    title: "10. Input, output, and failure"
  next:
    path: "/book/report.html"
    title: "12. Worked program: a combat replay"
---

# 11. Structured data and network requests

A level file or downloaded asset manifest begins as bytes or text, not as evidence that a game's type requirements hold.
A [codec](../dictionary.md#codec) connects a representation with a value of an expected type.
The important boundary is where the program checks that the external representation satisfies its model.

## Decoding a known shape

An annotation determines the type expected from JSON decoding:

```ruddy
type LevelSettings = { name: String, max_enemies: Nat }

let read_level_settings: String -> Result LevelSettings std::json::Error = std::json::decode
let level_settings = read_level_settings "{\"name\":\"arena\",\"max_enemies\":3}"
```

`level_settings` contains `#Some` with the decoded struct.
The same decoder rejects a missing `max_enemies` field, an unexpected field, or a negative value where `Nat` is required.
The result remains a `Result`; a static type annotation does not make an arbitrary level file valid.
A `Nat` field also does not enforce a sensible enemy budget: a game-specific maximum still needs a runtime check.

The default JSON representation is structural and strict.
Structs are objects, arrays are arrays, and tagged alternatives use an object containing their tag and payload.
Tuples use objects with numeric field labels, so a tuple should not be assumed to encode as a JSON array.
The [JSON](../std/json.md) reference and [representation guide](../platform-apis.md#json) describe the precise formats and error details.

`json::parse` instead returns a JSON value for applications that must inspect a document before choosing its model.
That representation preserves number tokens and object members, including repeated names.
Parsing a document and decoding it into a domain value are distinct operations.

## Reusable codecs

The ordinary typed decoder obtains a [mirror](../dictionary.md#mirror) for the expected type and derives a default codec.
That mechanism does not require a program to inspect the mirror itself.
An explicit codec is useful when repeated save/load operations should reuse derivation or an existing asset format differs from the default.
[Codec](../std/codec.md) describes the encoder, decoder, and format-operation interfaces.
[Hidden types and mirrors](reflection.md) develops the runtime type information behind generic operations.

## Transport and application success

An HTTP request can succeed as a transport operation while returning an unsuccessful status.
This function keeps transport failure, status failure, and body handling separate:

```ruddy
let fetch_text = fn address => match std::http::get address with
| #Error error => #Error error.message
| #Some response =>
  if std::http::is_success response then #Some (std::http::text response)
  else #Error "The server returned an unsuccessful status"
  end
end
```

A successful text result is still only text.
Passing it through a typed JSON decoder creates a further boundary with its own failure cases.
The [HTTP](../std/http.md) interface buffers responses and exposes request limits; a program should choose limits appropriate to the data it expects.

## Constructing a request

A URL has structure, so query parameters should be encoded through [URL](../std/url.md) operations.
This example preserves two values for one query name:

```ruddy
let query_text = std::url::stringify_query [
  ("tag", "forest"),
  ("tag", "night"),
]
```

The result is `"tag=forest&tag=night"`.
When a parsed URL is changed, `with_query` produces a consistent updated URL rather than leaving its text and individual fields out of agreement.
HTTP headers and bodies similarly have API-defined representations; ordinary record spread customizes request defaults.

## Summary and exercises

Parsing, typed decoding, transport success, and application success establish different facts.
Keeping those boundaries visible makes errors understandable to callers.

1. Explain why valid JSON can still fail `read_level_settings`.
2. List the decisions required after an HTTP request returns status 404 with a valid JSON body.
3. Write a decoder for an array of records and exercise an empty array, a missing field, and an unexpected field.

[Selected answers](answers.md#external-data) distinguish the boundaries.

