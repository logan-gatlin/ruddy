# 19 Terminate descriptor construction for recursion through `hide`

Status: open
Type: task
Priority: P1

Reviewed baseline: `cd906cf`.

Spec: [Typed mirrors](../spec.md#1-typed-mirrors) and
[Compiler implementation and termination](../spec.md#5-compiler-implementation-and-termination).

## Problem and evidence

Requesting a mirror of a recursive type that crosses a hidden binder makes
descriptor construction expand indefinitely. The first review reproduced
compiler memory exhaustion in a bounded process. The latest review confirmed
that the faulty algorithm is unchanged; it did not repeat the exhausting run.

In [`Descriptor::from_graph_policy`](../../../src/reification.rs), the sharing
key is `(source_node, binders)`. Revisiting a recursive `hidden:` node pushes
another binder onto that list. The source graph is finite, but every traversal
then has a new key, so no back edge closes the descriptor graph.

## Minimal reproduction

Compile this standalone `main.rud` in a library with the manifest below:

```ruddy
type Option 'a = #None | #Some 'a
type Loop = hide 'a => { value: 'a, next: Option Loop }
@private extern mirror: () -> Mirror 'a = "$mirror"
let loop: Mirror Loop = mirror ()
```

```toml
name = "recursive-hide-repro"
version = "0.1.0"
kind = "library"
root = "main.rud"
target = "js"
platform = "node"
[dependencies]
std = false
```

The failure occurs during compilation, before executing the exported library.
Do not run the unfixed reproduction without memory and time limits.

## Required change

- Construct a finite semantic graph for supported recursive hidden types. The
  graph must preserve the binding relationship while recognizing recursive
  references; repeatedly extending the binder environment cannot manufacture
  infinitely many descriptor identities.
- Preserve alpha-equivalent bound-variable identity and distinguish independent
  free identities. Dropping all binder context from the cache key is not a
  sufficient fix.
- Ensure exact comparison and descriptor validation terminate on the resulting
  recursive graphs. Allocate recursive identities before connecting their edges
  as required by the spec.
- If a recursive form cannot yet be represented correctly, issue an explicit
  bounded compiler diagnostic. Resource exhaustion, a recovery descriptor, or
  falsely reporting unequal mirrors is not an acceptable fallback. An interim
  diagnostic must leave the unsupported spec requirement tracked as open.

## Acceptance checks

- The program above completes within a bounded time and memory budget. For
  supported types it produces a finite descriptor with recursion represented
  by back edges; independently verify the bound on node growth.
- Cover mutual recursion, nested hidden binders, an ordinary recursive type as
  a control, and a recursive type that refers to more than one surrounding
  binder. Validate the descriptor after construction and after artifact load.
- Compare alpha-equivalent recursive hidden types successfully, and reject
  equality for structurally different recursive types or different binding
  relationships. Keep these checks bounded too.
- Add a regression that cannot hang or exhaust the whole test process if the
  bug returns. Run Rust tests only through `just test`; preserve that
  recipe's process-tree memory limit and use an additional bounded child
  process where necessary for the failure regression.
