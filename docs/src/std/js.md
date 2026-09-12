---
doc: true
layout: std.njk
stdReference: true
---

# [std](bundle.md)::js

`js` converts and observes values at JavaScript boundaries.

A Ruddy record can be lowered into host data for JavaScript.

```ruddy
let host_user = std::js::lower { name: "Ada", active: true }
```

## Types

### Adapter

```ruddy
type Adapter 'a = {
  lower: 'a -> Result Value Error,
  lift: Value -> Result 'a Error + !Host,
  read: Value -> Result 'a Error,
}
```

The directions of one checked conversion between a Ruddy type and a host value. `lower` writes; `lift` reads an arbitrary host value, which observes it, so it carries the effect; `read` reads a snapshot, which runs no host code and so is pure. `adapter ()` is the structural adapter for whatever type the use site infers; a specialized adapter is an ordinary record a caller writes, and is used wherever the structural one would be.

### Error

```ruddy
type Error = ffi::DecodeError
```

Why a checked conversion stopped: `path` is the position inside the value, `expected` what that position had to be, and `message` the host's own words.

### Kind

```ruddy
type Kind =
  | #Null
  | #Undefined
  | #Bool
  | #Number
  | #String
  | #Array
  | #Object
  | #Function
  | #Other
```

What a host value is, as far as JavaScript itself reports. `#Null` and `#Undefined` stay apart, as do `#Array` and `#Object`; a symbol, a bigint, or a value whose class cannot be reported is `#Other`.

### Value

```ruddy
type Value = ForeignValue
```

A JavaScript value exactly as the host holds it. Ruddy never looks inside one: it is opaque, it cannot be built out of data, and forwarding it back to the host hands over the very same value, so the host's own `===` still holds. To read anything out of one, convert it with `lift` or observe it with `Host`.

## Effects

### Host

```ruddy
effect Host = {
  kind: Value -> Kind,
  field: { of: Value, name: String } -> Result Value Error,
  element: { of: Value, at: Nat } -> Result Value Error,
  length: Value -> Result Nat Error,
  keys: Value -> Result [String] Error,
  snapshot: Value -> Result Value Error,
  apply: { of: Value, arguments: [Value] } -> Result Value Error,
  text: Value -> Result String Error,
}
```

Observing a host value. Reading a property can run a getter or a proxy trap, so none of these is a pure read: they are operations of this effect, and a program that observes host values says so in its type. Every operation that can fail reports the host's own failure as an `Error` rather than throwing: a getter that raises, a proxy trap that refuses, or a value of the wrong shape comes back as `#Error`. `kind` cannot fail; a value it cannot report is `#Other`. `snapshot` copies inert data out of the host, and that copy can afterwards be read purely with `lift`; forwarding a `Value` instead keeps the host's identity.

## Values

### adapter

```ruddy
let adapter: () -> Adapter 'a
```

The structural adapter for the type this use site infers, made from the mirror the compiler supplies there. No direction carries a function contract.

### apply

```ruddy
let apply: [Value] -> Value -> Result Value Error + !Host
```

Call a host function with the given arguments: `apply [x] target`. A non-callable is refused. The call has no receiver, so read a method with `field` and let the host bind it if it needs one.

### element

```ruddy
let element: Nat -> Value -> Result Value Error + !Host
```

The element at an index: `element 0n value`.

### field

```ruddy
let field: String -> Value -> Result Value Error + !Host
```

The named property of a host value: `field "length" value`. The read runs whatever the host has put there.

### keys

```ruddy
let keys: Value -> Result [String] Error + !Host
```

The host value's own enumerable string keys, in the host's order.

### kind

```ruddy
let kind: Value -> Kind + !Host
```

What JavaScript reports this value to be.

### length

```ruddy
let length: Value -> Result Nat Error + !Host
```

The `length` property, which must be a finite non-negative integer inside this target's `Nat` domain.

### lift

```ruddy
let lift: Value -> Result 'a Error + !Host
```

Read an arbitrary host value as the type this use site infers, checking every position. Reading a property runs whatever the host put there, so this observes the host and says so in its type. Integers must be integral and inside the target's domain, text must be Unicode scalar values, and a callable is refused: rebuilding a function from data would be a contract nobody checked, so an extern declaration owns that.

### lower

```ruddy
let lower: 'a -> Result Value Error
```

Write a value of the inferred type as host data, or say where it could not be written. A function contract is never written: a callable is refused, and an explicit extern or callback adapter carries one instead.

### read

```ruddy
let read: Value -> Result 'a Error
```

Read a snapshot, or a host primitive, as the type this use site infers. A snapshot is a copy this program took, so nothing in it can run host code and this is an ordinary pure read. A live host value is refused here however plain it looks; observe it with `lift`, or take a `snapshot` first.

### snapshot

```ruddy
let snapshot: Value -> Result Value Error + !Host
```

An inert deep copy of plain host data: objects, arrays, and primitives. A function, a value already on the path being copied, and a host object that is not plain data are all refused. The copy is data nobody else holds, so reading it afterwards with `lift` is pure.

### text

```ruddy
let text: Value -> Result String Error + !Host
```

Read a host string, repairing it. Ruddy text is Unicode scalar values and a host string need not be, so `lift` refuses one that is not; this is the conversion that says instead what to do about it, replacing each unpaired half with `U+FFFD`. A value that is not a string at all is still refused.

<!-- Generated by ruddy doc for std. -->
