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
cross between the names. A function's mirror carries its effect contract,
so a function that performs an effect is never the same type as one that
performs none.

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

## ABI plans

`std::abi` writes a foreign calling contract down as ordinary data and checks
it. Backend-neutral ABI plans are validated against representative C and Wasm
layouts and invocation contracts. Production native/Wasm foreign integration is
later work; a validated plan is not a claim that those integrations already
execute. The module names no extern and declares no effect, so validating a
plan cannot call, allocate, or observe anything: it is a reading of the plan's
own numbers.

A `Plan` is a `Convention`, the address width in bits of the target it is
written for, its parameters, its result, and the obligations its adapter
contract carries. `validate : Plan -> Result () [Error]` answers with every
fault it can decide from the plan alone, rather than the first — a plan is
reviewed as a whole — and `report` renders those faults as plain English
sentences, each naming the position it is about. A `Type` is a scalar of an
explicit width and signedness, a pointer, a list, a string, a resource handle,
or a record with a `Layout`: a size, an alignment, and fields at their byte
offsets.

A layout is written down, never computed, so a header's real numbers can be
held against the rules. Validation rejects an alignment that is not a power of
two, a size the alignment does not divide, and a field that is misaligned for
its own type, overlaps the field before it, runs past the declared size, or is
aligned more strictly than the record holding it:

```ruddy
-- struct { char a; int b; double c; } on a 64-bit target.
let sample: std::abi::Layout = {
  name: "Sample",
  size: 16n,
  alignment: 8n,
  fields: [
    { name: "a", offset: 0n, shape: #Scalar (#Integer { bits: 8n, signed: true }) },
    { name: "b", offset: 4n, shape: #Scalar (#Integer { bits: 32n, signed: true }) },
    { name: "c", offset: 8n, shape: #Scalar (#Float { bits: 64n }) },
  ],
}
```

Moving `b` to byte 2 answers `the field "b" of the record "Sample" starts at
byte 2, which its own alignment of 4 bytes does not divide.`

Ownership and length travel with a pointer, a list, or a string, because
neither can be read off a type. An `Ownership` says which side allocates the
region, which side frees it, with what function, and how long it stays valid; a
`Length` names the parameter carrying the count, the count the contract fixes,
the sentinel that ends the region, or the count the canonical ABI passes beside
the pointer. A plan that omits either is refused.

`#C` and `#CanonicalAbi` are external ABIs to implement, not a description of
how Ruddy lays its own values out, and their differences are in the code rather
than between the lines. The C ABI carries no count beside a pointer, so a C
plan names the parameter, the fixed count, or the sentinel; the canonical ABI
passes a pointer and a count together, so a canonical plan does not choose
another way. The C ABI has neither a Unicode scalar value nor a resource
handle; the canonical ABI has no raw pointer and no 128-bit integer, and is
defined over 32-bit linear memory, with 64-bit reserved for memory64. The
[canonical ABI specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md)
is the contract those rules come from.

A type witness cannot make dereferencing an arbitrary C pointer safe. Validity,
length, lifetime, and allocation provenance must be supplied by the adapter
contract, so a plan records those obligations rather than discharging them: a
plan that passes a region of memory and states no validity or lifetime
obligation is refused, and one whose region is freed by a stated side must also
state its allocation provenance. Recording an obligation is not meeting it.

## JavaScript values

`std::js` is the JavaScript foreign-adapter layer: checked conversion in both
directions, and an effect for observing a host value that Ruddy has not
converted.

`js::Value` is the host's own value. It is opaque and cannot be built out of
data, and forwarding one back hands the host the very same value, so the host's
`===` still holds. `js::Error` is `ffi::DecodeError`: a `path` into the value,
what that position had to be, and the host's own `message`.

`js::lower : 'a -> Result Value Error` writes a value of the inferred type as
host data and `js::lift : Value -> Result 'a Error` reads one back, checking
every position. Both take the compiler's mirror for the type at the use site,
exactly as `reflect::mirror ()` does. Conversion is strict: an integer position
takes only an integral number inside the target's bound domain, so `1.5`, `-1`,
and a value past the domain's maximum are all refused; a negative zero from the
host becomes the one integer zero, while a `Real` keeps the sign the host gave
it; a text position takes only a string, and an astral scalar survives it.

`js::Adapter 'a` is the pair of directions as one value:

```ruddy
type Adapter 'a = { lower: 'a -> Result Value Error, lift: Value -> Result 'a Error }
```

`js::adapter ()` is the structural adapter for the inferred type. A specialized
adapter is an ordinary record a caller writes, and goes wherever the structural
one goes:

```ruddy
let millis: std::js::Value -> std::result::Result Nat std::js::Error = std::js::lift
let seconds: std::js::Adapter Nat = {
  lower: fn count => std::js::lower (std::nat::multiply count 1000n),
  lift: fn value => match millis value with
  | #Some elapsed => #Some (std::nat::divide elapsed 1000n)
  | #Error error => #Error error
  end,
}
```

Neither direction carries a function contract. A decoder that rebuilt a
callable would be promising behavior nobody checked, so `lift` refuses one and
says so: `a verifiable data type; a function contract needs an adapter`.

### Observing a host value

Reading a property of a host object can run a getter or a proxy trap, so it is
not a pure read. `js::Host` collects those operations, and a function that
performs them says so in its type:

```ruddy
let name_of: std::js::Value -> std::result::Result std::js::Value std::js::Error
  + std::js::!Host = std::js::field "name"
```

`kind value` reports what JavaScript says the value is, keeping `#Null` apart
from `#Undefined` and `#Array` apart from `#Object`; a symbol or a bigint is
`#Other`. `field name of` and `element at of` read a property and an index.
`length of` requires a finite non-negative integer `length` that this target's
`Nat` can hold. `keys of` returns the host's own enumerable string keys, in the
host's order. `apply arguments of` calls a callable with those arguments and no
receiver; read a method with `field` if the host must bind one. Each of these
reports the host's failure as an `#Error` carrying the host's own message rather
than throwing; only `kind` cannot fail, and reports `#Other` for a value it
cannot name.

A host value that throws when the runtime itself probes it, such as a revoked
proxy, never reaches these operations: that failure belongs to the calling
convention every extern shares, not to this module.

`snapshot of` copies plain data — objects, arrays, and primitives — out of the
host. It refuses a function, a value already on the path being copied, and a
host object that is not plain data, such as a `Date` or a `Map`. What comes back
is inert data nobody else holds, so reading it afterwards with `lift` is pure
and needs no `!Host`. Forwarding a `Value` instead of copying it keeps the
host's identity.

Node executables and Node or web libraries install the handler for `Host`, as
they do for the other platform effects. A local handler can answer these
operations without a host at all.

### Callbacks

A Ruddy function crosses to the host through an `extern` declaration, which
carries the complete per-arrow type, the effects the callback may perform, and
how the call completes:

```ruddy
extern each: fn(fn(Nat) -> Nat, [Nat]) -> [Nat] = "(step, values) => values.map(step)"
extern run_with: fn(fn(Nat) -> Nat + !Log) -> Nat + !Log = "step => step(21)"
@async
extern later: fn(fn(Nat) -> Nat, Nat) -> Nat =
  "(step, value) => new Promise(resolve => setTimeout(() => resolve(step(value)), 0))"
```

The callback's declared effects stay in the caller's type, so the caller still
handles them; `@async` says the host answers with a promise, and the Ruddy call
site reads as an ordinary call either way. Completion timing grants no further
permission. A declaration is also how a host function becomes a Ruddy one:
`extern make_adder: fn(Nat) -> fn(Nat) -> Nat` accepts the callable that `lift`
refuses.

Foreign code and its callers are trusted to respect the declared lifetime,
retention, thread, allocation, and effect contract; this layer adds no
revocation tokens or scoped-only callbacks. C and Wasm adapters need their own
opaque handle types and ABI contracts, which are not part of this module.
