---
doc: true
layout: std.njk
stdReference: true
---

# [std](bundle.md)::str

`str` inspects, transforms, searches, and combines text by Unicode scalar value.

Every plain occurrence of a substring can be replaced.

```ruddy
let redacted =
  std::str::replace_all ("Call 555-0100", "555-", "***-")
```

## Values

### char_at

```ruddy
extern char_at: fn(String, Nat) -> String
```

The scalar at a position, counting scalars from zero; empty past the end.

### char_at_option

```ruddy
let char_at_option: String -> Nat -> Option String
```

The scalar at the zero-based position, or `#None` when the position is outside the string.

### chars

```ruddy
let chars: String -> [String]
```

The scalar values as one-scalar strings.

### compare

```ruddy
let compare: order::Ordering String
```

Compares two strings by scalar value.

### concat

```ruddy
extern concat: fn(String, String) -> String
```

Joins two strings.

### contains

```ruddy
extern contains: fn(String, String) -> Bool
```

Whether the string contains the substring.

### drop

```ruddy
let drop: String -> Nat -> String
```

The text after the requested number of scalar values.

### ends_with

```ruddy
extern ends_with: fn(String, String) -> Bool
```

Whether the string ends with the suffix.

### equal

```ruddy
let equal: String -> String -> Bool
```

The Boolean comparisons derived from the three-way ordering.

### from_boolean

```ruddy
let from_boolean: Bool -> String
```

`"true"` for `true` and `"false"` for `false`.

### from_int

```ruddy
extern from_int: fn(Int) -> String
```

The decimal text of an integer.

### from_nat

```ruddy
extern from_nat: fn(Nat) -> String
```

The decimal text of a natural number.

### from_real

```ruddy
extern from_real: fn(Real) -> String
```

The text representation of a real number.

### greater_than

```ruddy
let greater_than: String -> String -> Bool
```

The Boolean comparisons derived from the three-way ordering.

### greater_than_or_equal

```ruddy
let greater_than_or_equal: String -> String -> Bool
```

The Boolean comparisons derived from the three-way ordering.

### index_of

```ruddy
extern index_of: fn(String, String) -> Int
```

The scalar position of the first occurrence of `search`, or -1.

### index_of_option

```ruddy
let index_of_option: String -> String -> Option Nat
```

The zero-based scalar position of the first occurrence, or `#None` when it is absent.

### is_blank

```ruddy
let is_blank: String -> Bool
```

Whether the string is empty or contains only whitespace.

### is_empty

```ruddy
let is_empty: String -> Bool
```

Whether the string contains no scalar values.

### join

```ruddy
let join: String -> [String] -> String
```

Joins strings with a separator between adjacent values.

### len

```ruddy
extern len: fn(String) -> Nat
```

The number of Unicode scalar values in the text: `"😀"` has length one.

### less_than

```ruddy
let less_than: String -> String -> Bool
```

The Boolean comparisons derived from the three-way ordering.

### less_than_or_equal

```ruddy
let less_than_or_equal: String -> String -> Bool
```

The Boolean comparisons derived from the three-way ordering.

### not_equal

```ruddy
let not_equal: String -> String -> Bool
```

The Boolean comparisons derived from the three-way ordering.

### pad_end

```ruddy
extern pad_end: fn(String, Nat, String) -> String
```

Pads the end to the requested scalar length.

### pad_start

```ruddy
extern pad_start: fn(String, Nat, String) -> String
```

Pads the beginning to the requested scalar length.

### repeat

```ruddy
extern repeat: fn(String, Nat) -> String
```

Repeats the string the supplied number of times.

### replace_all

```ruddy
extern replace_all: fn(String, String, String) -> String
```

Replace every occurrence of a plain substring, left to right and without overlapping. The replacement is inserted as written. An empty search matches at every scalar boundary, including before the first scalar and after the last.

### replace_first

```ruddy
extern replace_first: fn(String, String, String) -> String
```

Replace the first occurrence of a plain substring. The replacement is inserted as written: `$&` and its relatives are text, not a substitution. An empty search matches before the first scalar.

### reverse

```ruddy
extern reverse: fn(String) -> String
```

The scalar values in reverse order.

### slice

```ruddy
extern slice: fn(String, Nat, Nat) -> String
```

The scalars from `start` up to `end`, counting scalars from zero.

### slice_with

```ruddy
let slice_with: String -> { start when 'start: Nat, stop when 'stop: Nat } -> String
```

The scalars from `start` up to `end`, or `#None` when the positions are invalid.

### split

```ruddy
let split: String -> String -> [String]
```

Splits at every occurrence of a separator.

### split_once

```ruddy
let split_once: String -> String -> Option (String, String)
```

The text before and after the first separator, or `#None` when it is absent.

### starts_with

```ruddy
extern starts_with: fn(String, String) -> Bool
```

Whether the string begins with the prefix.

### strip_prefix

```ruddy
let strip_prefix: String -> String -> Option String
```

The text after a matching prefix.

### strip_suffix

```ruddy
let strip_suffix: String -> String -> Option String
```

The text before a matching suffix.

### take

```ruddy
let take: String -> Nat -> String
```

The first requested number of scalar values.

### to_lowercase

```ruddy
extern to_lowercase: fn(String) -> String
```

Converts cased scalars to lowercase.

### to_uppercase

```ruddy
extern to_uppercase: fn(String) -> String
```

Converts cased scalars to uppercase.

### trim

```ruddy
extern trim: fn(String) -> String
```

Removes whitespace from both ends.

### trim_end

```ruddy
extern trim_end: fn(String) -> String
```

Removes whitespace from the end.

### trim_start

```ruddy
extern trim_start: fn(String) -> String
```

Removes whitespace from the beginning.

### utf8_len

```ruddy
extern utf8_len: fn(String) -> Nat
```

How many bytes this text is as UTF-8. Scalars are what `len` counts; this is what a byte budget counts, and what crosses a binary boundary.

<!-- Generated by ruddy doc for std. -->
