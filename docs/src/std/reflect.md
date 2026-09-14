---
doc: true
layout: std.njk
stdReference: true
---

# [std](bundle.md)::reflect

`reflect` describes types as data through compiler-supplied mirrors.

A value's static type can be described as ordinary data.

```ruddy
let schema = std::reflect::describe (
  std::reflect::type_of { name: "Ada", active: true }
)
```

## Types

### ArrayView

```ruddy
type ArrayView 'array = hide 'element => {
  element: Mirror 'element,
  read: 'array -> ['element],
  make: ['element] -> 'array,
}
```

An array's element type, hidden, with the conversions between the array and a plain array of that element.

### Binding

```ruddy
type Binding 'record = hide 'field => {
  record: Mirror 'record,
  name: String,
  mirror: Mirror 'field,
  value: 'field,
}
```

A value for one field of a record, with the mirrors that prove which record and which field: a builder accepts it on those, never on where it was made.

### BuildError

```ruddy
type BuildError =
  | #Missing String
  | #Duplicate String
  | #Unknown String
  | #Foreign String
  | #Mismatched String
```

Why a record could not be built from bindings, naming the field.

### Description

```ruddy
type Description = { root: Nat, nodes: [Node] }
```

A finite graph describing a type: ordinary readable data, never evidence. Node indices refer into `nodes`; a recursive type is a graph with a back edge.

### Domain

```ruddy
type Domain = { bits: Nat, signed: Bool, min: String, max: String }
```

The exact domain of an integer kind: its precision in bits, its signedness, and its least and greatest values as decimal text, exact even where this target's own integers could not hold them.

### EmptyArrayView

```ruddy
type EmptyArrayView 'array = {
  element: Description,
  read: 'array -> [|],
  make: [|] -> 'array,
}
```

An array whose element type is proven empty. Its sole value is the empty array; the element description grants no constructor.

### Field

```ruddy
type Field = { name: String, node: Nat }
```

A named position of a record or sum node.

### InfoArrayView

```ruddy
type InfoArrayView 'array = hide 'element => {
  element: TypeInfo 'element,
  read: 'array -> ['element],
}
```

An array's descriptive element evidence and read operation.

### InfoRecordView

```ruddy
type InfoRecordView 'record = {
  mirror: TypeInfo 'record,
  fields: [InfoSomeField 'record],
}
```

A record's descriptive field views.

### InfoShape

```ruddy
type InfoShape 'a =
  | #Nat { read: 'a -> Nat }
  | #Int { read: 'a -> Int }
  | #Real { read: 'a -> Real }
  | #String { read: 'a -> String }
  | #Bool { read: 'a -> Bool }
  | #Nat8 { read: 'a -> Nat8 }
  | #Nat16 { read: 'a -> Nat16 }
  | #Nat32 { read: 'a -> Nat32 }
  | #Nat64 { read: 'a -> Nat64 }
  | #Int8 { read: 'a -> Int8 }
  | #Int16 { read: 'a -> Int16 }
  | #Int32 { read: 'a -> Int32 }
  | #Int64 { read: 'a -> Int64 }
  | #Array (InfoArrayView 'a)
  | #Record (InfoRecordView 'a)
  | #Sum (InfoSumView 'a)
  | #Function Description
  | #Hidden Description
  | #Mirror Description
  | #TypeInfo Description
  | #Foreign
```

Typed observation without construction authority.

### InfoSomeCase

```ruddy
type InfoSomeCase 'sum = hide 'payload => {
  name: String,
  mirror: TypeInfo 'payload,
  project: 'sum -> Option 'payload,
}
```

A case's descriptive payload evidence and projection, without injection authority.

### InfoSomeField

```ruddy
type InfoSomeField 'record = hide 'field => {
  name: String,
  mirror: TypeInfo 'field,
  presence: Presence,
  read: 'record -> Option 'field,
}
```

A field's descriptive evidence and read operation, sharing one hidden field type.

### InfoSumView

```ruddy
type InfoSumView 'sum = { mirror: TypeInfo 'sum, cases: [InfoSomeCase 'sum] }
```

Every described case, including cases with impossible payloads.

### Node

```ruddy
type Node =
  | #Nat Domain
  | #Int Domain
  | #Real
  | #String
  | #Bool
  | #Foreign
  | #Fixed Domain
  | #Array Nat
  | #Cell { region: Nat, element: Nat }
  | #Function { argument: Nat, result: Nat, effects: [Field] }
  | #Effects [Field]
  | #Record [Field]
  | #Sum [Field]
  | #Alias Nat
  | #Extend { base: Nat, rest: Nat }
  | #Parameter Nat
  | #Mirror Nat
  | #TypeInfo Nat
  | #Hidden Nat
  | #Variable Nat
```

One node of a description. Integer kinds carry their domain, so `Nat` and `Nat64` stay apart even where their domains coincide; a function reports its argument and result; records and sums list their fields by name.

### Presence

```ruddy
type Presence = #Required | #Optional
```

Whether a record field is always there. A mirror describes one exact type, whose fields are all required; the case is kept so a reader written against views handles absence as well.

### RecordView

```ruddy
type RecordView 'record = {
  mirror: Mirror 'record,
  fields: [SomeField 'record],
  build: [Binding 'record] -> Result 'record BuildError,
}
```

A record's fields, each with its own hidden type, and a checked builder from bindings back to the record.

### Shape

```ruddy
type Shape 'a =
  | #Nat { read: 'a -> Nat, make: Nat -> 'a }
  | #Int { read: 'a -> Int, make: Int -> 'a }
  | #Real { read: 'a -> Real, make: Real -> 'a }
  | #String { read: 'a -> String, make: String -> 'a }
  | #Bool { read: 'a -> Bool, make: Bool -> 'a }
  | #Nat8 { read: 'a -> Nat8, make: Nat8 -> 'a }
  | #Nat16 { read: 'a -> Nat16, make: Nat16 -> 'a }
  | #Nat32 { read: 'a -> Nat32, make: Nat32 -> 'a }
  | #Nat64 { read: 'a -> Nat64, make: Nat64 -> 'a }
  | #Int8 { read: 'a -> Int8, make: Int8 -> 'a }
  | #Int16 { read: 'a -> Int16, make: Int16 -> 'a }
  | #Int32 { read: 'a -> Int32, make: Int32 -> 'a }
  | #Int64 { read: 'a -> Int64, make: Int64 -> 'a }
  | #Array (ArrayView 'a)
  | #EmptyArray (EmptyArrayView 'a)
  | #Record (RecordView 'a)
  | #Sum (SumView 'a)
```

Constructive structural views: primitives, records, arrays, empty-only arrays, and sums. Nested mirrors satisfy the same construction contract. Functions, cells, foreign values, hidden packages, and evidence types have no automatic constructors; inspect them with `InfoShape` instead.

### SomeCase

```ruddy
type SomeCase 'sum = hide 'payload => {
  name: String,
  mirror: Mirror 'payload,
  project: 'sum -> Option 'payload,
  inject: 'payload -> 'sum,
}
```

One realizable case, with its payload type hidden. `project` observes the payload and `inject` constructs the case directly. Proven impossible cases are omitted here and retained in `describe`; unsupported but possibly inhabited cases prevent obtaining the mirror.

### SomeField

```ruddy
type SomeField 'record = hide 'field => {
  name: String,
  mirror: Mirror 'field,
  presence: Presence,
  read: 'record -> Option 'field,
  bind: 'field -> Binding 'record,
}
```

One field of a record with its type hidden: opening it gives one scoped type shared by the mirror, what `read` observes, and what `bind` accepts.

### SumView

```ruddy
type SumView 'sum = { mirror: Mirror 'sum, cases: [SomeCase 'sum] }
```

A sum's cases, each with its own hidden payload type.

## Values

### construct

```ruddy
extern construct: Mirror 'a -> 'a
```

Executes the mirror's finite pure construction. Primitive values are zero, false, or empty; records construct every field, arrays are empty, and variants choose a case of minimum construction height, breaking ties by canonical case order.

### describe

```ruddy
extern describe: Mirror 'a -> Description
```

A mirror's structure as ordinary data. Editing the description grants nothing: it cannot be turned back into a mirror.

### describe_info

```ruddy
extern describe_info: TypeInfo 'a -> Description
```

The complete type description, including impossible cases and opaque positions.

### info

```ruddy
extern info: Mirror 'a -> TypeInfo 'a
```

A constructive mirror's descriptive evidence.

### info_of

```ruddy
extern info_of: 'a -> TypeInfo 'a
```

Descriptive evidence for a value's static type. Does not capture the value or grant constructors.

### mirror

```ruddy
extern mirror: () -> Mirror 'a
```

Authentic construction evidence for an inferred type. An annotation, use, or caller selects the type. The compiler requires accessible constructors for every potentially inhabited part and a finite pure construction; empty types and unsupported constructors are compile errors. Generic callers retain this requirement.

### same

```ruddy
let same: Mirror 'a -> Mirror 'b -> Option { forward: 'a -> 'b, backward: 'b -> 'a }
```

Whether two mirrors are one type, exactly: equal structure, primitives, and effect contracts. On success the two functions convert nothing and return their argument unchanged.

### same_info

```ruddy
let same_info: TypeInfo 'a -> TypeInfo 'b -> Option { forward: 'a -> 'b, backward: 'b -> 'a }
```

Exact type equality from authenticated descriptive evidence; converts no values and grants no constructors.

### shape

```ruddy
extern shape: Mirror 'a -> Shape 'a
```

The outermost structure of a mirror, with typed operations on values of its type. Nested types are reached through the mirrors the views carry, one level per call, so a recursive type is inspected as far as a program looks.

### shape_info

```ruddy
extern shape_info: TypeInfo 'a -> InfoShape 'a
```

Read-only typed structure. Observing a type does not grant construction authority.

### type_info

```ruddy
extern type_info: () -> TypeInfo 'a
```

Authenticated descriptive evidence for an inferred static type, without construction authority.

### type_of

```ruddy
extern type_of: 'a -> Mirror 'a
```

A constructive mirror of a value's static type. It requires the same construction evidence as `mirror`; the value grants no constructor authority. Use `info_of` for observation alone.

<!-- Generated by ruddy doc for std. -->
