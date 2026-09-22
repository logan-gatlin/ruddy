# Presence signature probes

These checks ran on 2026-09-21 using the **existing** executable at
`/var/home/dev/code/hc/ruddy/target/debug/ruddy`. They describe that executable's
behavior; they were not run against a fresh build of the working tree.

Each check used `ruddy check` in an isolated temporary project, with
`kind = "library"`, `target = "artifact"`, and `[dependencies] std = false`.
Function bodies were copied from `/var/home/dev/code/hc/img/src/main.rud`
unchanged. The first project omitted the image aliases and final `test` binding
and removed `@private`; the second copied only the function definitions needed
for each probe. No source file in either repository was changed. No Rust tests
were run.

## First batch

Project: `/tmp/ruddy-presence-probes-qaz_co0i`.
Each expression below was appended to the copied definitions as `let probe = …`.
The original copied definitions were restored after the batch.

| # | Expression | Result |
| --- | --- | --- |
| 1 | `get_channel #Red {r: 1n, g: "green"}` | Rejected: natural number/text mismatch. |
| 2 | `get_channel #Red {r: 1n, label: "green"}` | Accepted. |
| 3 | `set_channel #Red 1n {g: "green"}` | Rejected: natural number/text mismatch. |
| 4 | `set_channel #Red 1n {r: "old"}` | Rejected: natural number/text mismatch. |
| 5 | `set_channel #Red 1n {}` | Accepted. |
| 6 | `extend4 (1n,)` | Rejected: tagged value/natural number mismatch. |
| 7 | `extend4 ("hello",)` | Rejected: tagged value/text mismatch. |
| 8 | `swizzle (#Red,) (#Green,) {r: 1n}` | Rejected: required presence missing. |
| 9 | `swizzle (#Red,) (#Green,) {r: 1n, g: 2n}` | Accepted. |
| 10 | `swizzle (#Red,) (#Green,) {r: 1n, g: 2n, b: "blue"}` | Rejected: natural number/text mismatch. |
| 11 | `try_get_set #None #None {r: 1n, g: "green"}` | Rejected: natural number/text mismatch. |
| 12 | `try_get_set #None #Red {}` | Rejected: required presence missing. |
| 13 | `try_get_set #Red #None {}` | Rejected: required presence missing. |

## Second batch

Project: `/tmp/ruddy-presence-small-cjxpya3e`.
Checks 14–17 use this explicitly widened selector before the expression:

```ruddy
let selector: #Red | #Green = #Red
```

| # | Expression | Result |
| --- | --- | --- |
| 14 | `get_channel selector {r: 1n, g: 2n}` | Accepted. |
| 15 | `get_channel selector {r: 1n}` | Rejected: required presence missing. |
| 16 | `set_channel selector 1n {}` | Rejected: required presence missing. |
| 17 | `set_channel selector 1n {r: 2n, g: 3n}` | Accepted. |
| 18 | The partial application and two uses below. | Accepted. |
| 19 | `extend4 (#Some 1n,)` | Accepted. |
| 20 | `extend4 (#None 1n,)` | Rejected: struct/natural number mismatch; `#None` has a unit payload. |

```ruddy
let red = set_channel #Red
let a = red 1n {}
let b = red "hello" {}
```

## Exact simplification of extend4

Project: `/tmp/ruddy-extend4-equations-bwglmzva`.
The original `extend4` body was annotated with the signature from
`current-signatures.md`, replacing only its 25 `where` clauses with:

```ruddy
where
  'input_0_3 <= 'input_0_2 <= 'input_0_1 <= 'input_0_0;
  'output_0_none = (not 'input_0_0 or 'input_0_0_none);
  'output_1_none = (not 'input_0_1 or 'input_0_1_none);
  'output_2_none = (not 'input_0_2 or 'input_0_2_none);
  'output_3_none = (not 'input_0_3 or 'input_0_3_none)
```

`ruddy check` accepted the unchanged body with this annotation.

An independent Python parser/evaluator for the printed Boolean grammar then
compared the original conjunction against this simplified conjunction on all
**4,096 assignments of the 12 presence variables**. They agreed on every
assignment; both admitted 80 assignments. The simplification is an exact
equivalence of the presence constraints, not merely a more restrictive
annotation that happens to check.

## Consequences for readable alternatives

- All present fields named `r`, `g`, `b`, or `a` share one payload type, including
  fields unselected by this particular call. Unrelated row-tail fields can
  retain other types. A path signature that puts unselected color fields into
  an unconstrained row tail promises more than current inference provides.
- `set_channel` can add a field for a known singleton selector. A mixed selector
  requires its possible result shapes to agree. The current type system does
  not create a general union of `{r: T}` and `{g: T}` records for the mixed call.
- `extend4` accepts sums in its occupied slots, including payload-bearing tags,
  but reserves the `#None` tag for its nullary form. An overload using arbitrary
  whole-type variables would be an inference extension, not faithful printing.
- The apparent no-op branches of `try_get_set` still inherit the shared image
  type skeleton and the field requirements of admitted color tags. A general
  `#None -> 'anything -> 'image -> 'image` overload would overpromise.
- `swizzle` preserves the input image's shape and requires all color fields
  mentioned by either channel tuple. It currently cannot add a destination
  field. Its runtime behavior is sequential: later transfers read the image
  updated by earlier transfers, so a type-level account must not assume a
  simultaneous permutation.
- A case notation must retain mixed selector types and survive partial
  application and independent let-instantiation. Singleton-only overloads
  alone do not establish those properties.
