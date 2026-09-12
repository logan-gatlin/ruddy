---
doc: true
bookNavigation:
  previous:
    path: "/book/modules.html"
    title: "7. Organizing programs"
  next:
    path: "/book/state.html"
    title: "9. Mutable state and regions"
---

# 8. Effects and handlers

A movement or damage calculation can be checked with a fixed world and expected next state.
A simulation that also reads the clock, samples player input, or writes a combat log needs decisions about its environment before the same check is repeatable.
Passing those decisions through every helper by hand would couple the calculation to the surrounding application.

An [effect](../dictionary.md#effect) can name an operation the calculation needs, and an [effect handler](../dictionary.md#effect-handler) supplies its behavior.
The same simulation can then run with a live clock during play and recorded times during a replay.
Its type records the effects still visible to its caller.

## Purity and substituting results

A [pure function](../dictionary.md#pure-function) depends only on its inputs and fixed captured values, with no externally observable effects.
For the same inputs, a normally completing call can be replaced by its result without changing the program's behavior.
That makes it possible to reason about one calculation at a time or deliberately cache an expensive result.
It does not mean the compiler automatically caches calls, or that the calculation necessarily terminates.

A clock-reading function does not have that property on its own: repeated calls can return different times.
A logging function may return the same unit value every time but still change what is printed.
Effects make these differences visible in the interface rather than leaving callers to discover them by reading the implementation.

## An operation and a handler

A clock separates the need for a time value from its source:

```ruddy
effect Clock = { now: () -> Nat }

let timestamp: () -> Nat + !Clock = fn _ => !Clock.now ()

let example_time = handle timestamp () with
| !Clock.now _ => 100n
end
```

The function calls the clock operation with unit.
The handler supplies `100n`, so `example_time` is `100n`.
The operation result resumes the computation at the call that requested it.
If more computation followed that call, it would receive the supplied value.

The type `() -> Nat + !Clock` records the operation that a caller must arrange to handle.
The local handler discharges that requirement for this expression.
A clock handler returning a string would violate the declared operation's result type.

## A pure wrapper around an effectful function

A replayable simulation needs the clock dependency to become an ordinary input.
This wrapper supplies a chosen time whenever its argument asks for the clock:

```ruddy
let at_time: Nat -> (() -> 'a + !Clock) -> 'a + | = fn now computation =>
  handle computation () with
  | !Clock.now _ => now
  end

let fixed_timestamp = at_time 100n timestamp

let can_attack = fn ready_at => std::nat::greater_than_or_equal (!Clock.now ()) ready_at
let ready_this_tick = at_time 100n (fn _ => can_attack 90n)
let waiting_this_tick = at_time 100n (fn _ => can_attack 120n)
```

The timestamp is `100n`, `ready_this_tick` is `true`, and `waiting_this_tick` is `false`.
A replay or a check chooses the current tick without changing the cooldown rule.
The callback has a clock effect, but the wrapper's final `+ |` states that its application has no remaining effects.
Its handler answers from `now`, an ordinary input, and introduces no new operation.
The effectful implementation is therefore usable through a pure interface without rewriting it.

Handling an effect does not automatically make a function pure.
A handler that reads the host clock or prints output has introduced another observable operation, which must remain in the surrounding contract.
Purity concerns the whole wrapped computation, including its handlers.

## Sequencing output

A `do` block can sequence operations and return an ordinary result.
This function prints two messages, discards the unit returned by the first call, and returns the second call's unit:

```ruddy
let announce = fn message => do
  _ = println "Turn started"
  return println message
end
```

Both calls perform the standard [IO](../std/io.md) effect.
The runtime normally handles it for an executable.
The prelude's `IO` name is an alias usable in types; naming operations in a handler requires the concrete declaration.

A local handler can modify output before forwarding it to the surrounding handler:

```ruddy
using std::io::IO

let prefixed = fn message => handle println message with
| !IO.write text => !IO.write (std::str::concat "Combat: " text)
| !IO.write_error text => !IO.write_error text
end
```

Calling the handled effect inside an operation arm forwards to an outer handler rather than re-entering the same handler.
`prefixed` therefore still requires `IO` outside itself.
A named operation interface is closed: a handler supplies every operation, including `write_error` here.

## Rethrowing and composing log handlers

Logging often has several independent policies: add a match label, add an AI-system label, and decide where messages go.
A handler can implement one policy and rethrow the operation for the next handler to interpret.
Rethrowing means performing the same operation from its handler arm; it is distinct from `raise`, which abandons the handled computation.

A logging interface and prefix handler isolate the labeling policy:

```ruddy
effect Log = { write: String -> () }

let with_prefix = fn prefix computation => handle computation () with
| !Log.write message => !Log.write (std::str::concat prefix message)
end

let simulate_ai = fn _ => !Log.write "target acquired"
let logged_tick = fn _ =>
  with_prefix "match: " (fn _ => with_prefix "ai: " simulate_ai)
```

The innermost handler turns `"target acquired"` into `"ai: target acquired"` and rethrows.
The outer prefix handler receives that message and rethrows `"match: ai: target acquired"`.
Neither handler chooses an output destination.
If a rethrown operation re-entered the same handler, the prefix would be added forever; proceeding to the next outer handler lets each policy contribute once.

A final handler chooses to send the messages to the console:

```ruddy
let to_console: (() -> 'a + !Log) -> 'a + !IO = fn computation =>
  handle computation () with
  | !Log.write message => println message
  end

let run_logged_tick = fn _ => to_console logged_tick
```

The annotation on `to_console` shows the change in requirements: its callback needs `Log`, while applying the wrapper needs `IO`.
`run_logged_tick ()` prints `match: ai: target acquired` followed by a newline.
The `Log` requirement has been handled, but the resulting function still has the console's `IO` effect.
A caller that wants no log output can instead use a handler that answers the operation without forwarding it:

```ruddy
let without_logs: (() -> 'a + !Log) -> 'a + | = fn computation =>
  handle computation () with
  | !Log.write _ => ()
  end

let quiet_tick = without_logs logged_tick
```

The last definition produces unit with no output or remaining effect.
Both wrappers reuse the same logging computation; choosing a destination and composing labels remain separate decisions.

## Completion and early exit

A handler's `return` arm transforms normal completion.
This example supplies a clock value and adds one after the handled expression completes:

```ruddy
let next_time = handle timestamp () with
| !Clock.now _ => 100n
| return value => std::nat::add value 1n
end
```

The result is `101n`.
This `return` arm belongs to the handler, distinct from `return` in a `do` block.

Inside an operation arm, `raise expression` ends the handled computation instead of resuming the operation.
The following check completes with `"unavailable"` before the normal result can be produced:

```ruddy
effect Available = () -> Bool

let availability = handle
  (if !Available () then "ready" else "waiting" end)
with
| !Available _ => raise "unavailable"
end
```

`raise` belongs to an enclosing handler arm and cannot cross into a nested function.
Its purpose here is a handler-controlled exit; ordinary recoverable failures can still be modeled with [Result](../std/result.md).

## Effects that cannot resume

The `Available` operation asks for a `Bool`, so a handler can either supply one and resume or use `raise` to stop.
Sometimes continuing is not a valid response at all: a computation has been cancelled, or a failure means its remaining steps must not run.
The operation's result type can express that requirement:

```ruddy
effect Cancel = () -> |

let cancelled_turn = handle do
  _ = !Cancel ()
  return "turn resolved"
end with
| !Cancel _ => raise "turn cancelled"
end
```

`()` is the input type: cancellation needs no payload.
The result type `|` is the empty sum, with no cases and therefore no value a handler could return to resume the operation.
It differs from `()`, which has one value and can acknowledge an operation that continues normally.
It also differs from `+ |` on a function type, which says that the function has no effects; here `|` describes the operation's result.

`cancelled_turn` is `"turn cancelled"`.
The handler exits the handled computation with that string, so the `return "turn resolved"` after the operation never runs.
Returning `()` from the handler arm would be a type error: unit is not a value of the empty sum.
A handler can also rethrow the operation to an outer handler, but that outer handler still cannot supply a normal resumption value.
This does not force the whole process to terminate: `raise` can turn the stopped computation into an ordinary result at the chosen handler boundary.

The standard [Halt](../std/effects.md) effect generalizes this pattern to carry a value: `Halt 'a` has an operation of type `'a -> |`.
That payload can describe why the computation stopped or supply its early result.
[Process exit](../std/process.md) also has an empty result type, because a terminated process cannot continue at the exit call.

## Effects that cannot be handled

A handler can replace an operation only when the computation makes a request through that operation.
A foreign function that reads the host clock or changes external state directly has already chosen how to perform that behavior.
A Ruddy handler cannot turn that direct observation or change into a pure calculation.

An effect declaration can have no operations at all, as in `effect Immediate`.
The standard library uses this form for [std::ffi::Immediate](../std/ffi.md).
It can appear in a function's effect contract, but there is no operation to call with `!Immediate` and no operation arm a Ruddy handler can provide to remove it.
It records an effect without exposing a replaceable request.

For example, this foreign declaration binds a direct JavaScript clock read:

```ruddy
extern host_time: fn(()) -> Real + std::ffi::!Immediate = "() => Date.now()"

let read_host_time = fn _ => host_time ()
```

`extern` connects the name to the supplied host implementation; the [interoperability chapter](interoperability.md#declaring-a-host-operation) explains the calling convention.
`read_host_time` returns a number normally, but retains `Immediate` in its effect contract.
Wrapping the call in a `handle` expression does not remove that effect, and an annotation claiming the wrapper is pure would be rejected.
There is no suspended clock request for the handler to answer.

To make such behavior replaceable, expose an application operation such as `Clock.now` and have the real handler perform the foreign read.
The real handler retains the foreign effect; a test handler can instead supply a fixed value without reading the host.
The pure interface comes from choosing an implementation that avoids the foreign observation, not from intercepting `Immediate` after it has happened.

The two special cases address different questions:

| Declaration | Can an operation resume normally? | Can Ruddy supply an operation handler? |
| --- | --- | --- |
| `effect Clock = { now: () -> Nat }` | Yes, with a `Nat`. | Yes. |
| `effect Cancel = () -> |` | No result value exists to resume with. | Yes; it can exit or forward the operation. |
| `effect Immediate` | No operation is declared; a foreign function carrying this effect may still return normally. | No operation is available to intercept. |

## Why top-level initialization restricts effects

Top-level definitions can refer to each other without having to be written in dependency order.
Mutually recursive functions make the need clear: neither function can always be placed before the other.
This pair defines evenness and oddness through one another:

```ruddy
using std::nat

let even = fn number =>
  if nat::is_zero number then true else odd (nat::subtract number 1n) end

let odd = fn number =>
  if nat::is_zero number then false else even (nat::subtract number 1n) end
```

Reversing these definitions does not change their meaning.
If initializing each definition could also print, write a file, or observe changing state, their order would become part of the program's observable behavior.
With mutually dependent initializers, deciding which effect must happen first would no longer follow a simple dependency order.

Ruddy therefore prohibits escaping effects in top-level initialization so that definitions can remain unordered.
A declaration can contain an effectful function, but the operations happen when that function is called.
Placing those calls in `main` or another function makes their sequence explicit where the behavior is needed.
Reordering declarations then does not silently reorder application operations.

The restriction is on effects that remain outside the initializer.
A locally handled operation can still compute a top-level value, as `example_time` does with its fixed clock handler.
Its handler supplies the operation without an observable dependency on the outside world.
This keeps the freedom to define values using useful abstractions while preserving the reason for the restriction.

## Effects in higher-order functions

An array mapping operation inherits the effects of its callback.
Mapping a pure movement function is pure; mapping an animation function that reads the clock requires a clock handler.
The [Array](../std/array.md) signatures express this relationship with an open effect row.
[Rows](rows.md) explain how the relationship is named.

Effects on curried functions attach to a particular arrow.
`A -> B -> C + !Clock` places the effect on the call that supplies `B`.
`A -> (B -> C) + !Clock` places it on the call that supplies `A`.
Partial application alone therefore does not always perform the effects of the final call.

## Summary and exercises

An operation describes a request, its handler supplies behavior, and a function type records the effects that remain.
An empty operation result rules out normal resumption; an effect without operations records behavior that Ruddy handlers cannot replace.

1. Trace `next_time`, separating the operation result from the handler's normal result.
2. Replace the fixed clock value and predict which expressions change.
3. Explain why the forwarding output handler still has an output effect.
4. Supply `true` from the `Available` arm instead of raising and predict the result.
5. Reverse the two prefix handlers and derive the new message.
6. Explain why `at_time` and `without_logs` are pure while `to_console` is not.
7. Explain how effectful initialization would make reordering mutually dependent definitions observable.
8. Change `Cancel`'s result type from `|` to `()` and return `()` from its handler arm. Predict the result and explain why it changes.
9. Explain why a fixed `Clock` handler can make a wrapper pure, while wrapping `host_time` cannot remove its `Immediate` effect.

[Selected answers](answers.md#effects) compare resumption, early exit, and effects that cannot be intercepted.

