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

### Field

```ruddy
type Field = { name: String, node: Nat }
```

A named position of a record or sum node.

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
  | #Record (RecordView 'a)
  | #Sum (SumView 'a)
  | #Function Description
  | #Hidden Description
  | #Mirror Description
  | #Foreign
```

The outermost structure of a mirrored type, as views whose operations read and make values of that type. Cells use the opaque Foreign shape and expose no read or write operations. Primitive cases carry the typed conversions; the descriptive cases carry the type's description and no way to make one.

### SomeCase

```ruddy
type SomeCase 'sum = hide 'payload => {
  name: String,
  mirror: Mirror 'payload,
  project: 'sum -> Option 'payload,
  inject: Option ('payload -> 'sum),
}
```

One case of a sum with its payload type hidden. `project` observes the payload when the value is this case; `inject` makes the case, when the mirror proves it may be made.

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

### describe

```ruddy
extern describe: Mirror 'a -> Description
```

A mirror's structure as ordinary data. Editing the description grants nothing: it cannot be turned back into a mirror.

### mirror

```ruddy
extern mirror: () -> Mirror 'a
```

Authentic evidence for the type this position is inferred at. The compiler supplies it: an annotation, a use, or a caller decides the type, never a runtime value.

### same

```ruddy
let same: Mirror 'a -> Mirror 'b -> Option { forward: 'a -> 'b, backward: 'b -> 'a }
```

Whether two mirrors are one type, exactly: equal structure, primitives, and effect contracts. On success the two functions convert nothing and return their argument unchanged.

### shape

```ruddy
extern shape: Mirror 'a -> Shape 'a
```

The outermost structure of a mirror, with typed operations on values of its type. Nested types are reached through the mirrors the views carry, one level per call, so a recursive type is inspected as far as a program looks.

### type_of

```ruddy
extern type_of: 'a -> Mirror 'a
```

The mirror of a value's static type at this use. It does not inspect the value.

<!-- Generated by ruddy doc for std. -->
