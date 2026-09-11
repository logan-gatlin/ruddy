---
doc: true
---

# Ruddy

Ruddy is a [functional language](dictionary.md#functional-language) with [static typing](dictionary.md#static-typing).
Programs combine functions to transform data, and types describe which values those functions accept and return.
[Type inference](dictionary.md#type-inference) allows types to be omitted when they can be determined from the code.

## Functions and values

`let` gives a value a name, and `fn` defines a function.
Functions are values: they can be passed to other functions and returned as results.
A function call places its argument after the function name.
For example, `display_name` extracts a name from a [struct](dictionary.md#struct), a value containing named fields:

```ruddy
let display_name = fn person => person.name
let name = display_name { name: "Ada", email: "ada@example.com" }
```

The result bound to `name` is `"Ada"`.
The function needs a `name` field but does not need to know about the `email` field.

## Types and data

Ruddy uses [structural typing](dictionary.md#structural-typing): [type compatibility](dictionary.md#type-compatibility) depends on a type's structure, rather than the name of its [definition](dictionary.md#definition).
`type` gives a type a name, and `:` supplies an explicit type for a value.
For example, a value fits `Contact` because it has the required field with the required type:

```ruddy
type Contact = { email: String }
let contact: Contact = { email: "ada@example.com" }
```

A [tag](dictionary.md#tag) identifies a case and can carry a value.
[Pattern matching](dictionary.md#pattern-matching) selects an expression based on the shape of a value and gives names to its parts.
For example, `email_or_default` uses an address carried by `#Some`, or a default address for `#None`:

```ruddy
let email_or_default = fn email => match email with
| #Some address => address
| #None => "support@example.com"
end
```

## Effects

An [effect](dictionary.md#effect) describes an operation whose behavior is supplied by an [effect handler](dictionary.md#effect-handler).
Effects are tracked in function types, including when those types are inferred.
This allows code to request operations such as console output while a handler determines how those operations are performed.

For example, `main` prints a greeting through the standard library's `IO` effect:

```ruddy
let main = fn _ => println "Hello!"
```

An executable starts by calling `main` with `()`, the [unit](dictionary.md#unit) value.
The [prelude](standard-library.md#prelude) makes `println` available without qualification.
Here, `println` writes the text followed by a newline and returns `()`.
The runtime supplies the `IO` effect handler; a program can also supply its own handlers.

## Getting started

The [download page](download.md) covers installation and editor setup.
The [platform API guide](platform-apis.md) covers JSON, process information, paths, HTTP requests, URLs, and mirrors.

The following reading order builds on this introduction:

1. [Hello, World!](hello-world.md)
2. [Grammar](grammar.md)
3. [Standard library](standard-library.md)
