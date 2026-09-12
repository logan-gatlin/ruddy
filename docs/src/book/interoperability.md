---
doc: true
bookNavigation:
  previous:
    path: "/book/reflection.html"
    title: "14. Hidden types, mirrors, and generic operations"
  next:
    path: "/book/tooling.html"
    title: "Appendices"
---

# 15. JavaScript interoperability

A [foreign value](../dictionary.md#foreign-value) connects Ruddy to an implementation supplied by the target environment.
At that boundary, a type declaration is a contract with [host](../dictionary.md#host) code outside Ruddy's type checker.
The contract must describe both the values exchanged and any observable operations.

## Declaring a host operation

An `extern` declaration supplies a type and a target expression.
An editor tool can reuse a pure JavaScript string transformation when formatting an enemy name:

```ruddy
extern uppercase: fn(String) -> String = "text => text.toUpperCase()"
let loud_name = uppercase "slime"
```

The result is `"SLIME"`.
`fn(String) -> String` describes a foreign calling convention; ordinary Ruddy code still calls `uppercase` by placing its argument after the function.
The target expression is JavaScript and makes this declaration specific to that target.

An operation that directly observes external state must declare the [Immediate](../std/ffi.md) effect.
For example, a host clock cannot be described as a pure constant-producing function:

```ruddy
extern host_time: fn(()) -> Real + std::ffi::!Immediate = "() => Date.now()"
```

This effect [cannot be handled in Ruddy](effects.md#effects-that-cannot-be-handled): it records a direct foreign observation and declares no operation a handler could intercept.
A replaceable application clock should expose an ordinary [effect interface](effects.md), with its host observation supplied at the outer boundary.

## Checked data conversion

Opaque JavaScript values use `std::js::Value`.
Lowering a Ruddy value produces host data when its representation is supported; reading it back checks an expected type:

```ruddy
let round_trip: Nat -> Result Nat std::js::Error = fn count =>
  match std::js::lower count with
  | #Error error => #Error error
  | #Some host_value => std::js::read host_value
  end
```

A host primitive such as this number can be read without observing a live object.
For a live object, property access might invoke getters or proxy behavior.
`js::lift` therefore has a `Host` effect, while `js::read` requires a host primitive or a [snapshot](../dictionary.md#snapshot).
Taking a snapshot first observes the host and produces inert data that can subsequently be read purely.
The distinction depends on the value's provenance, not merely whether it looks like a plain object.
[JS](../std/js.md) documents those operations and their structured errors.

## Callbacks and completion

A foreign declaration can accept a Ruddy callback with an explicit function contract.
This declaration invokes a pure callback on each natural number:

```ruddy
extern host_map: fn(fn(Nat) -> Nat, [Nat]) -> [Nat] =
  "(step, values) => values.map(step)"

let next_counts = host_map (fn count => std::nat::add count 1n) [1n, 2n]
```

The result is `[2n, 3n]`.
A callback that performs effects needs those effects represented in the declaration and surrounding call contract.
A generic data decoder cannot establish a function's behavioral contract by inspecting its JavaScript shape.

Completion timing is another part of the contract.
`@async` declares a promise-returning host operation; Ruddy call sites remain ordinary calls.
A declaration must match the host behavior, rather than treating a promise as an ordinary result value.
A promise-returning identity operation demonstrates the completion contract without changing the value:

```ruddy
@async
extern later_count: fn(Nat) -> Nat = "value => Promise.resolve(value)"
```

A call such as `later_count 3n` produces `3n` after the host promise completes.
The [callback guide](../platform-apis.md#callbacks) and [foreign syntax](../grammar.md#modules-attributes-and-foreign-values) cover the detailed forms.

## Exports and platforms

A JavaScript library exposes public values through host-callable adapters.
`@export "sync"` and `@export "promise"` select fixed completion contracts when an interface requires them.
The compiler checks whether the selected platform can handle an export's effects.
An executable's root `main` is called by its entry adapter; a library's exports are not automatically invoked as an application entry point.

The [tooling appendix](tooling.md#targets-and-platforms) distinguishes build targets from platforms.
Conditional definitions can select the implementation for a target or platform while retaining one intended application interface.
[ABI](../std/abi.md) validates explicit calling-layout plans, but validation alone does not execute native or Wasm integration.
A valid plan also does not establish that a foreign pointer's lifetime or allocation is correct.

## Summary and exercises

A foreign boundary needs explicit value, effect, callback, and completion contracts.
Checked data conversion establishes a representation, while an adapter owns callable behavior.

1. Explain why the string transformation and host clock need different effect contracts.
2. Compare reading a host number with reading a property of a live object.
3. Explain why declaring a callback's argument and result types is not enough when the callback performs output.

[Selected answers](answers.md#interoperability) identify the observable operations.

