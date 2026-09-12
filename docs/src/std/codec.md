---
doc: true
layout: std.njk
stdReference: true
---

# [std](bundle.md)::codec

`codec` defines format-independent encoders, decoders, and their protocols.

A default codec can be derived for a record type.

```ruddy
type Settings = { theme: String, retries: Nat }

let settings_codec: Result (std::codec::Codec Settings) std::codec::DeriveError =
  std::codec::derive (std::reflect::mirror ())
```

## Types

### Codec

```ruddy
type Codec 'a = { encoder: Encoder 'a, decoder: Decoder 'a }
```

An encoder and decoder for one type, sharing the same schema.

### Cursor

```ruddy
type Cursor = { frame: Nat }
```

A position within the session a reader or writer keeps. It is valid only while its frame is the innermost open one; a stale cursor is a protocol error, never a way past the session's checks.

### Decoder

```ruddy
type Decoder 'a = { schema: reflect::Description, run: () -> Result 'a Error + !Read }
```

How to read a value through the protocol, with a description of what it reads.

### DeriveError

```ruddy
type DeriveError = { path: [TypeStep], reason: String }
```

Why a type has no default codec, with the path to the position that has none.

### Encoder

```ruddy
type Encoder 'a = { schema: reflect::Description, run: 'a -> Result () Error + !Write }
```

How to write a value through the protocol, with a description of what it writes.

### Entry

```ruddy
type Entry = hide 'a => { id: String, mirror: Mirror 'a, codec: Codec 'a }
```

One type a registry encodes and decodes dynamically, under an application-owned identity: its mirror and its codec, hidden together so that the codec is known to be the mirror's.

### Error

```ruddy
type Error = { path: [Step], kind: Failure }
```

Why encoding or decoding failed, and the path to the value it failed at.

### Failure

```ruddy
type Failure =
  | #Parse { message: String, offset: Nat }
  | #Missing String
  | #Duplicate String
  | #Unknown String
  | #UnexpectedCase String
  | #Unexpected { expected: String, found: String }
  | #Range { expected: String, found: String }
  | #Protocol String
  | #Limit String
  | #Cycle
  | #Unsupported String
  | #Custom String
```

What went wrong: the input could not be parsed; a field was missing, repeated, or unknown; a case was not one of the type's; the input held another kind of value than the type asked for; a number was out of its domain; the protocol was misused; a limit was reached; or a codec of its own said so.

### RecordSchema

```ruddy
type RecordSchema = { fields: [String] }
```

The fields a record has on the wire, in order. A format that carries names reads them; one that does not takes the order from here.

### Registry

```ruddy
type Registry = [Entry]
```

The types a program chooses to write and read as `Any`: a wire identity never manufactures a mirror, so only a registered type can come back with its own.

### Step

```ruddy
type Step = #Field String | #Index Nat | #Case String
```

One step of the path to a value within a document: which field, element, or case it is under.

### TypeStep

```ruddy
type TypeStep = #Field String | #Element | #Case String
```

One step of the path to a position within a type: where derivation stopped.

### VariantSchema

```ruddy
type VariantSchema = { cases: [String] }
```

The cases a sum has on the wire.

## Effects

### Read

```ruddy
effect Read = {
  read_bool: () -> Result Bool Error,
  read_nat: () -> Result Nat Error,
  read_int: () -> Result Int Error,
  read_nat64: () -> Result Nat64 Error,
  read_int64: () -> Result Int64 Error,
  read_real: () -> Result Real Error,
  read_text: () -> Result String Error,
  skip_value: () -> Result () Error,
  begin_record: RecordSchema -> Result Cursor Error,
  next_field: Cursor -> Result (Option String) Error,
  end_record: Cursor -> Result () Error,
  begin_sequence: () -> Result Cursor Error,
  next_element: Cursor -> Result Bool Error,
  end_sequence: Cursor -> Result () Error,
  begin_variant: VariantSchema -> Result { cursor: Cursor, tag: String } Error,
  end_variant: Cursor -> Result () Error,
}
```

The protocol a decoder reads through. Every operation is typed by what it asks for; the format's handler owns the input and a checked session stack, and answers a request out of order with a protocol error.

### Write

```ruddy
effect Write = {
  write_bool: Bool -> Result () Error,
  write_nat: Nat -> Result () Error,
  write_int: Int -> Result () Error,
  write_nat64: Nat64 -> Result () Error,
  write_int64: Int64 -> Result () Error,
  write_real: Real -> Result () Error,
  write_text: String -> Result () Error,
  begin_record: RecordSchema -> Result Cursor Error,
  write_field: { cursor: Cursor, name: String } -> Result () Error,
  end_record: Cursor -> Result () Error,
  begin_sequence: Nat -> Result Cursor Error,
  next_element: Cursor -> Result () Error,
  end_sequence: Cursor -> Result () Error,
  begin_variant: { schema: VariantSchema, tag: String } -> Result Cursor Error,
  end_variant: Cursor -> Result () Error,
}
```

The protocol an encoder writes through. The format's handler owns the output and a checked session stack.

## Values

### at

```ruddy
let at: [Step] -> Result 'a Error -> Result 'a Error
```

An error at a path: what an operation reported, placed at the value being worked on.

### decode_any

```ruddy
let decode_any: Registry -> () -> Result any::Any Error + !Read
```

Read an `Any` written by `encode_any`, back as the registered type it was: the identity selects the entry, the entry's codec reads the value, and the entry's mirror is what the `Any` carries.

### derive

```ruddy
let derive: Mirror 'a -> Result (Codec 'a) DeriveError
```

The default codec of a type, from its mirror. See `derive_encoder`.

### derive_decoder

```ruddy
let derive_decoder: Mirror 'a -> Result (Decoder 'a) DeriveError
```

The default decoder of a type, from its mirror. See `derive_encoder`.

### derive_encoder

```ruddy
let derive_encoder: Mirror 'a -> Result (Encoder 'a) DeriveError
```

The default encoder of a type, from its mirror: the structural one, which writes primitives as themselves, arrays as sequences, records as records of their fields, and sums as variants. A type with a function, hidden type, mirror, or foreign value in it has none, and the error says where.

### encode_any

```ruddy
let encode_any: Registry -> any::Any -> Result () Error + !Write
```

Write an `Any` as a record of the identity its type is registered under and the value, when the registry has an entry whose mirror is the same type.

### entry

```ruddy
let entry: String -> Mirror 'a -> Codec 'a -> Entry
```

A registry entry with an explicit codec.

### fail

```ruddy
let fail: [Step] -> Failure -> Result 'a Error
```

An error of a codec's own, at a path.

### register

```ruddy
let register: String -> Mirror 'a -> Result Entry DeriveError
```

A registry entry with the type's default codec.

### validate

```ruddy
let validate: reflect::Description -> Result () DeriveError
```

Whether a type has a default codec: primitives, arrays, records, sums, and regular recursion among them. Functions, cells, hidden types, mirrors, and foreign values do not, and the error says at which position.

<!-- Generated by ruddy doc for std. -->
