---
doc: true
---

# JSON, process, path, HTTP, and URLs

JSON, URL, and HTTP work in Node executables and in libraries targeting Node or
web. Web executables still require a web entry adapter. Process and path effects
have Node handlers; a web library can use process operations under a local
handler. `std::path` is available for the Node platform.

Operations use ordinary Ruddy function calls even when they suspend. Immutable
records, arrays, tagged unions, and `Result` values form the public API; host
objects and promises do not escape into application code.

## JSON

`std::json::encode : 'a -> Result String Error` writes a value of the inferred
type as a JSON document, and `std::json::decode : String -> Result 'a Error`
reads one back. Both obtain the type's mirror and derive its default codec, so
they need no annotation beyond the one that decides the type:

```ruddy
let decode_config: String -> Result { name: String, retries: Nat } std::json::Error =
  std::json::decode
```

The default profile is strict and structural: records are objects, arrays are
arrays, sums are `{ "tag": ..., "value": ... }` objects, unit is `{}`, and
tuples are objects with their numeric labels. Typed decoding refuses unknown,
repeated, and missing fields, anything after the one document, and a number
outside its type's domain: an integer accepts any integral token, `1e3` and
`100e-2` among them, and a `Real` is correctly rounded, with overflow and a
nonzero underflow to zero refused and negative zero kept. Errors say whether
the type has no default codec (`#Derive`, with a path into the type) or what
went wrong where (`#Codec`, with a path of fields, indexes, and cases into the
value): a parse failure with its offset, a missing, duplicate, or unknown
field, an unexpected case, a value of another kind than the type asked for, a
range failure, a protocol misuse, or a limit.

`encode_with` and `decode_with` take an explicit `codec::Encoder` or
`codec::Decoder` and `Limits` on input length, nesting depth, member count,
and number digits, refused before anything is allocated. A codec derived with
`std::codec::derive` may be reused across calls.

`std::json::parse : String -> Result Value Error` reads a document as data
instead, keeping members in order, repeated keys among them, and numbers as
their exact tokens:

```ruddy
type Value =
  | #Null
  | #Bool Bool
  | #Number Number
  | #String String
  | #Array [Value]
  | #Object [(String, Value)]
```

`number_to_real`, `number_to_nat`, and `number_to_int` convert a token under
the same checked policies. `stringify` writes a value back, preserving member
order, repeats, and number spellings after checking that each spells a JSON
number. `get name value` looks up the first member with a name.

## Codecs

`std::codec` is the portable layer under JSON: `Read` and `Write` are effects
whose operations are typed by what they carry, so an encoder is a function
`'a -> Result () Error + !Write` and a decoder `() -> Result 'a Error + !Read`.
A format is a handler for those effects owning its input or output and a
checked session stack, and a buffered runner such as JSON's discharges the
protocol purely. `derive : Mirror 'a -> Result (Codec 'a) DeriveError` gives
the structural codec of any type made of primitives, arrays, records, sums, and
regular recursion; it interprets the mirror's views, so a recursive type is
handled one level at a time. A custom codec bypasses derivation and speaks the
protocol itself, and a misordered or incomplete session is a protocol error.

`std::binary` runs the same codecs over a positional binary format: no names
or tags on the wire, only what the decoder's schema says comes next, with
`Nat` and `Int` as 64 bits, text as a length and UTF-8, sequences as a count,
and cases as their index. `encode_versioned` and `decode_versioned` put an
application-owned schema identity and version before the value, so a reader
refuses another schema before reading any of it.

Dynamic values go through a caller-selected `codec::Registry`: `encode_any`
writes an `Any` as the identity its type is registered under and the value, and
`decode_any` reads one back as the registered type, with the entry's own mirror.
A wire identity never manufactures a mirror, so an unregistered type is an
error in both directions. `json::encode_canonical` writes the named canonical
profile, with members sorted by key and no whitespace, for hashes and
signatures.

## Process

`std::process::Process` exposes:

| Operation | Result |
| --- | --- |
| `args ()` | `[String]`, excluding the Node executable and script path |
| `env name` | `Result (Option String) Error` |
| `cwd ()` | `Result String Error` |

An absent environment variable is `#Some #None`; an empty value is
`#Some (#Some "")`. Invalid names are errors. `Error` contains `kind`
(`#InvalidName`, `#Unavailable`, or `#Other`) and `message`.

`std::process::exit code` performs the existing `Exit` effect and never returns.
The Node handler retains its output-draining behavior and saturates codes to 255.

Tests or embedded applications can supply a process without touching the host:

```ruddy
let arguments = fn _ => handle std::process::args () with
| std::process::!Process.args _ => ["--verbose"]
| std::process::!Process.env _ => #Some #None
| std::process::!Process.cwd _ => #Some "/virtual"
end
```

## Paths

`std::path` uses the host's Node path conventions. Its `posix` and `windows`
submodules provide explicit conventions independent of the host:

| Function | Type |
| --- | --- |
| `join` | `[String] -> String` |
| `normalize`, `basename`, `dirname`, `extension` | `String -> String` |
| `is_absolute` | `String -> Bool` |
| `parse` | `String -> { root, dir, base, ext, name }`, all fields strings |

These lexical operations are pure; loading Node's path module can suspend.
They do not access the filesystem or resolve symlinks. An extension includes the
leading dot; an absent extension is the empty string.

`path::resolve parts` and `path::relative from to` use Node's native path rules,
including its current-directory and Windows drive-directory behavior. They
perform `path::Path` and return `Result String { message: String }`, making their
ambient dependencies replaceable with a local handler. The effect's named
`relative` operation takes `{ from, to }`.

## URLs

`std::url::parse text` and `std::url::resolve base reference` return
`Result Url { input: String, message: String }`. Parsing requires an absolute
URL; resolving accepts a relative reference against an absolute base.

`Url` is a snapshot containing string fields `href`, `origin`, `protocol`,
`username`, `password`, `host`, `hostname`, `port`, `pathname`, `search`, and
`hash`. It follows WHATWG URL normalization. Snapshots are ordinary records;
edits are not automatically validated or reflected into their other fields.
`with_query` reparses the snapshot's `href` and returns a fresh, consistent URL.

Queries are immutable `[(String, String)]` arrays. Duplicate names and ordering
are preserved:

```ruddy
let query = std::url::parse_query "q=hello+world&q=again"
let values = std::url::get_all "q" query
let updated = std::url::set "page" "2" query
let encoded = std::url::stringify_query updated
```

`get name query` returns the first value as an `Option`. `get_all` returns all
matches; `append` adds a pair at the end; `remove` removes all matches; `set`
removes all matches and appends one replacement. These functions preserve their
inputs. Parsing and stringifying follow URLSearchParams form encoding (`+` for
space). `query url` parses its `search`; `with_query url query` replaces the query
while preserving the rest of `href`.

## HTTP

`std::http::request : Request -> Result Response Error` performs `http::Http`.
Node and web-library host adapters install a Fetch-based handler. Local handlers
can return responses without networking. A non-2xx HTTP status is a successful
transport result; `http::is_success response` checks for 200–299.

Use record spreads to customize defaults:

```ruddy
let send = fn url => std::http::request {
  method: "POST",
  headers: [("content-type", "application/json")],
  body: #Text "{\"name\":\"Ruddy\"}",
  timeout_ms: 5000n,
  ..std::http::defaults url
}
```

`Request` contains `url`, `method`, `headers`, `body`, `timeout_ms`, `max_bytes`,
and `redirect`. `defaults url` selects GET, empty headers, `#Empty` body, a
30-second timeout, a 16 MiB response limit, and `#Follow` redirects. Bodies are
`#Empty | #Text String | #Bytes [Nat8]`; redirects are
`#Follow | #Manual | #Error`. `get url` uses defaults;
`post url body` changes the method and body.

Only absolute HTTP/HTTPS URLs are accepted. The timeout must be between 1 and
2,147,483,647 milliseconds and covers both response headers and body consumption.
`max_bytes` must be a safe natural number; zero allows an empty response only.
The limit counts bytes delivered by Fetch, after host decompression. On timeout,
body-read failure, or excess size, the adapter aborts the request and releases
the reader. Responses are buffered; this API does not provide streaming or
caller-initiated cancellation.

`Response` contains `status: Nat`, `status_text: String`,
`headers: [(String, String)]`, `body: [Nat8]`, `url: String`, and `redirected: Bool`.
`text response` decodes UTF-8 using Fetch semantics, replacing malformed sequences
and stripping a BOM. `json response` performs typed JSON decoding.
`header name headers` does a case-insensitive lookup and returns an `Option`.
Response headers follow the host's Fetch normalization and visibility rules;
separate Set-Cookie entries are retained when the host exposes `getSetCookie`.

HTTP errors contain `kind`, the requested `url`, and `message`. Kinds are
`#InvalidRequest`, `#Network`, `#Timeout`, `#TooLarge`, and `#Unsupported`.
Redirect rejection is a network error, matching Fetch. Browser CORS, forbidden
headers, cookie visibility, and opaque manual redirects retain their normal
Fetch behavior. Credentials use Fetch's default `same-origin` policy.

The host contracts follow the official
[Node path](https://nodejs.org/api/path.html),
[Node process](https://nodejs.org/api/process.html),
[Fetch](https://developer.mozilla.org/en-US/docs/Web/API/Window/fetch), and
[URL](https://developer.mozilla.org/en-US/docs/Web/API/URL) documentation.

## Mirrors

`std::reflect` works with `Mirror 'a`, a builtin type whose values are the
compiler's own evidence for a type. `reflect::mirror : () -> Mirror 'a` makes
one at whatever type the position is inferred at, and `reflect::type_of : 'a ->
Mirror 'a` makes one for a value's static type without inspecting the value.
A mirror cannot be built from data: foreign code cannot forge one, and
`ffi::decode` accepts only a mirror the compiler made.

`reflect::describe : Mirror 'a -> Description` renders a mirror as an ordinary
finite graph of nodes, so a program can inspect a type's primitives, fields,
tags, and function shapes. `reflect::same : Mirror 'a -> Mirror 'b -> Option {
forward: 'a -> 'b, backward: 'b -> 'a }` decides whether two mirrors are exactly
one type, and on success supplies the two identity functions that let a value
cross between the names.

Together with [hidden types](dictionary.md#hidden-type) this recovers a value's
type at runtime; the standard `Any` is defined exactly this way, as
`hide 'a => { mirror: Mirror 'a, value: 'a }`, with `any::upcast` packaging a
value and `any::downcast` opening one:

```ruddy
type Dynamic = hide 'a => { value: 'a, evidence: Mirror 'a }
let as_nat: Dynamic -> Option Nat = fn item => match item with
| hide 'x { value, evidence } => match std::reflect::same evidence (std::reflect::mirror ()) with
  | #Some { forward, .. } => #Some (forward value)
  | #None => #None
  end
end
```

Inside the arm, a mirror the pattern bound is evidence for the opened type, so
`std::reflect::type_of value` and generic foreign calls on `value` work there.

`reflect::shape : Mirror 'a -> Shape 'a` gives a mirror's outermost structure
as typed views. Primitive cases carry `read : 'a -> Nat` and `make : Nat -> 'a`
and their like, so a generic library converts without a cast; `#Array`,
`#Record`, and `#Sum` carry views whose parts each hide their own type:

```ruddy
type SomeField 'record = hide 'field => {
  name: String,
  mirror: Mirror 'field,
  presence: Presence,
  read: 'record -> Option 'field,
  bind: 'field -> Binding 'record,
}
```

Opening a field with `hide 'f { mirror, read, bind, .. }` gives one scoped type
shared by the mirror, what `read` observes, and what `bind` accepts, so a
generic printer, validator, or decoder can work on a field's value and hand a
new one back without knowing the field's type. A record view's `build` takes
such bindings and rejects a missing, duplicate, unknown, or mismatched field,
or one bound for another record. A sum case's `project` observes its payload
and `inject` makes the case; functions, hidden types (`Any` among them),
mirrors, and foreign values are described only.
