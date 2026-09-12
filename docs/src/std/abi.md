---
doc: true
layout: std.njk
stdReference: true
---

# [std](bundle.md)::abi

`abi` validates C and WebAssembly component calling contracts.

A scalar-only C calling plan can be validated for a 64-bit target.

```ruddy
let plan: std::abi::Plan = {
  name: "absolute_value",
  target: { convention: #C, address_bits: 64n, scalar_alignment: #None },
  parameters: [{
    name: "value",
    shape: #Scalar (#Integer { bits: 32n, signed: true }),
  }],
  result: #Some (#Scalar (#Integer { bits: 32n, signed: true })),
  obligations: [],
}

let validation = std::abi::validate plan
```

## Types

### Convention

```ruddy
type Convention = #C | #CanonicalAbi
```

The external ABI a plan is written against. `#C` is a platform C ABI, and `#CanonicalAbi` is the WebAssembly component model's canonical ABI. Both are external ABIs to implement; neither describes how Ruddy lays its own values out.

### Duty

```ruddy
type Duty = #Validity | #Length | #Lifetime | #Provenance | #Thread | #Completion
```

A duty the adapter contract must supply, because no type witness supplies it.

### Encoding

```ruddy
type Encoding = #Utf8 | #Utf16 | #Latin1OrUtf16 | #Other String
```

How text is spelled in memory. The canonical ABI's own encodings are UTF-8, UTF-16, and latin1-or-UTF-16, and a canonical plan states one of those; C has no encoding of its own, so a C contract states with `#Other` the one it actually uses.

### Error

```ruddy
type Error = { position: String, reason: Reason }
```

One fault in a plan and the position it is about, such as `the field "b" of the record "Sample"`. `message` renders it as a plain English sentence; the reason is the machine-readable part.

### Field

```ruddy
type Field = { name: String, offset: Nat, shape: Type }
```

One field of a record, at its byte offset from the start of the record. Fields are written in offset order.

### Layout

```ruddy
type Layout = { name: String, size: Nat, alignment: Nat, fields: [Field] }
```

A record's layout as a foreign header really declares it: the total size in bytes including trailing padding, the alignment in bytes, and the fields at their offsets. A layout is written down and checked, never computed, so that a plan can be held against the numbers the other side already uses.

### Length

```ruddy
type Length = #Parameter String | #Field String | #Fixed Nat | #Sentinel String | #Canonical
```

How the callee learns how many elements travel with a pointer. Every pointer, list, and string states one; a pointer to a single object states `#Fixed 1n`, because a pointer whose readable extent nobody wrote down is exactly what this plan is for. `#Parameter` names another parameter of the same plan that carries the count, `#Field` names a field beside it in the same record, as `struct iovec` counts `iov_base` by `iov_len`, `#Fixed` states a count the contract fixes, `#Sentinel` names the terminator that ends the region, and `#Canonical` is the count the canonical ABI passes beside the pointer.

### Lifetime

```ruddy
type Lifetime = #Call | #UntilFreed | #Static
```

How long a region stays valid: for the call only, until whoever frees it does, or for the life of the program.

### Obligation

```ruddy
type Obligation = { duty: Duty, note: String }
```

An obligation the adapter contract carries, with the note that says how it is met. Recording an obligation is not discharging it; validating a plan only checks that the plan states the ones its own shapes require.

### Ownership

```ruddy
type Ownership = {
  allocated_by: Side,
  freed_by: Option Side,
  free_with: Option String,
  lifetime: Lifetime,
}
```

Who allocates a region, who frees it, and with what, since none of it follows from a type. `free_with` names the foreign function that releases the region, because a region taken from one allocator and returned to another is a fault no witness catches. A region nobody frees states `#None`, which is a claim about static or arena storage rather than a guess.

### Parameter

```ruddy
type Parameter = { name: String, shape: Type }
```

One parameter of a call, by name so that a length can refer to it.

### Plan

```ruddy
type Plan = {
  name: String,
  target: Target,
  parameters: [Parameter],
  result: Option Type,
  obligations: [Obligation],
}
```

A foreign calling contract as data: the target it is written for, the parameters, the result, and the obligations the adapter must supply. Nothing here executes a call.

### Reason

```ruddy
type Reason =
  | #Alignment Nat
  | #ScalarAlignment Nat
  | #Size { size: Nat, alignment: Nat }
  | #Offset { offset: Nat, alignment: Nat }
  | #Overlap { offset: Nat, previous: String, ends: Nat }
  | #PastEnd { offset: Nat, size: Nat, declared: Nat }
  | #UnderAligned { needs: Nat, declared: Nat }
  | #NoOwner
  | #NoLength
  | #UnknownLength String
  | #UnknownField String
  | #ZeroWidth
  | #Width { bits: Nat, convention: Convention }
  | #Unsupported { construct: String, convention: Convention, instead: String }
  | #LengthConvention Convention
  | #NoLayout { construct: String, convention: Convention }
  | #MissingDuty Duty
  | #AddressWidth { bits: Nat, convention: Convention }
```

What is wrong with a plan, as data. Each case is decidable from the plan alone: nothing here reports anything a foreign call would have to run to discover.

### Scalar

```ruddy
type Scalar =
  | #Integer { bits: Nat, signed: Bool }
  | #Float { bits: Nat }
  | #Bool
  | #Char
```

A scalar as the ABI spells it, with its width in bits and its signedness written down rather than inferred from a Ruddy type. `#Char` is the component model's Unicode scalar value; the C ABI has no such type.

### Side

```ruddy
type Side = #Caller | #Callee
```

Which side of a foreign call a duty falls on: the Ruddy caller, or the foreign callee.

### Target

```ruddy
type Target = { convention: Convention, address_bits: Nat, scalar_alignment: Option Nat }
```

The target a plan is written for: the ABI it follows, the address width in bits, and the largest alignment that target gives a scalar. `scalar_alignment` is `#Some 4n` for i386 System V, which aligns a double to 4 bytes rather than to its own 8, and `#None` for a target such as x86-64 or AArch64 that aligns every scalar to its own width. It is stated rather than derived because no rule recovers it from the address width.

### Type

```ruddy
type Type =
  | #Scalar Scalar
  | #Pointer { pointee: Type, ownership: Option Ownership, length: Option Length, nullable: Bool }
  | #List { element: Type, ownership: Option Ownership, length: Option Length }
  | #Text { encoding: Encoding, ownership: Option Ownership, length: Option Length }
  | #Handle { resource: String, owned: Bool }
  | #Record Layout
```

What a position carries across the boundary. A pointer, list, or string carries its ownership and its length with it, because neither can be read off a type.

## Values

### address_widths

```ruddy
let address_widths: Convention -> [Nat]
```

The address widths in bits a convention is defined over. The canonical ABI's MVP is defined over 32-bit linear memory, with 64-bit reserved for memory64. A C target narrower than 64 bits usually caps scalar alignment as well, which its target states.

### alignment_of

```ruddy
let alignment_of: Target -> Type -> Option Nat
```

How many bytes a value of this shape is aligned to on a target, or nothing when that target's convention lays it out nowhere. A record is aligned as its layout declares. Everything else is aligned to its own width, capped by the target's `scalar_alignment`, which is how i386 System V aligns a double to 4 bytes and not to 8.

### check_layout

```ruddy
let check_layout: Target -> Layout -> [Error]
```

Every fault a record's layout has on a target: an alignment that is not a power of two, a size the alignment does not divide, and any field that is misaligned for its own type, overlaps the field before it, runs past the declared size, or is aligned more strictly than the record that holds it. A layout is checked here without a plan around it, so a field whose length names a parameter is left alone; a field whose length names another field is still resolved, because its siblings are right here.

### construct_name

```ruddy
let construct_name: Type -> String
```

What a position is, as a sentence names it.

### convention_name

```ruddy
let convention_name: Convention -> String
```

A convention's name, as a sentence uses it.

### duty_name

```ruddy
let duty_name: Duty -> String
```

A duty's name, as a sentence uses it.

### float_widths

```ruddy
let float_widths: Convention -> [Nat]
```

The floating-point widths in bits a convention passes.

### integer_widths

```ruddy
let integer_widths: Convention -> [Nat]
```

The integer widths in bits a convention passes. The component model has u8, u16, u32, and u64 and no wider integer; a C target that has `__int128` passes it in a register pair.

### message

```ruddy
let message: Error -> String
```

A plain English sentence for one fault, naming the position it is about.

### report

```ruddy
let report: Plan -> [String]
```

Every fault of a plan as plain English sentences, one per fault, in the order `validate` reports them.

### size_of

```ruddy
let size_of: Target -> Type -> Option Nat
```

How many bytes a value of this shape occupies in memory on a target, or nothing when that target's convention lays it out nowhere. A list or a string is one object under the canonical ABI, a pointer beside a separate count under C.

### validate

```ruddy
let validate: Plan -> Result () [Error]
```

Every fault a plan has that can be decided from the plan alone, or nothing. All of them are reported at once, not the first: a plan is reviewed as a whole, and a reviewer who is told about one bad offset at a time fixes the same layout four times. A plan that validates is a plan whose own numbers and ownership hold together. It is not a working foreign call: production native and Wasm integration is later work, and nothing here executes anything.

<!-- Generated by ruddy doc for std. -->
