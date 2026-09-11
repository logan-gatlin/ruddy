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

`std::json::parse : String -> Result Value Error` returns this recursive value:

```ruddy
type Value =
  | #Null
  | #Bool Bool
  | #Number Real
  | #String String
  | #Array [Value]
  | #Object [(String, Value)]
```

Use pattern matching to inspect arbitrary JSON, or `json::get name value` to
look up an object member. `json::stringify` accepts a `Value` and returns
`Result String Error`. `Error` contains a `message`.

For known data, `json::decode : String -> Result 'a DecodeError` uses the expected
Ruddy type as its schema, including nested records and arrays:

```ruddy
let decode_config:
  String -> Result { name: String, retries: Nat } std::json::DecodeError =
  std::json::decode
```

Its errors distinguish `#Parse { message }` from
`#Decode { path, expected, message }`. Decoding uses `ffi::decode` and its existing
native-data rules; JSON null is represented explicitly by `#Null` in `Value`,
and is not automatically converted to `Option` in typed decoding. Tagged unions
in typed decoding use the existing foreign `{ tag, value }` representation.

Parsing follows JavaScript JSON semantics: duplicate object keys retain the last
value, integer-like keys follow JavaScript property order, and numbers use
binary64 precision. Non-finite numbers (including overflowing input such as
`1e400`) are errors. Stringifying a constructed object preserves its member
sequence, including duplicate keys; `get` returns the first matching member.
Parse errors also include host recursion-limit failures for deeply nested data.

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
