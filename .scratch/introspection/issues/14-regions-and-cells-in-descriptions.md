# 14 Regions, cells, and hidden dependencies in descriptions

Status: open
Type: task
Blocked by: 03

Spec: "Descriptions include ... region and presence dependencies", and "A cell
or callable can appear as an opaque value with useful type metadata."

`reification::Node` has no region node and no cell node, and a type containing
`mut` is refused outright rather than described as an opaque value.

Separately, the spec says the compiler "must reject existential packaging
whose hidden dependencies cannot yet be tracked safely" and that "Hiding an
ordinary type does not authorize hiding a region lifetime." `Solve::witness`
checks unification-variable levels only, so a witness naming a local region
rigid is packaged without complaint. Conservative rejection, with a diagnostic
and a fixture, is the tracked work.

## Comments

Implementer handoff at `cd906cf`: the safety gap is reproduced by this source,
which currently checks successfully:

```ruddy
type Box = hide 'a => 'a
@private let pack: 'a -> Box = fn value => value
let escape: () -> Box = fn _ => pack (mut 0n)
```

The local cell's region disappears from the outward package and effect
contract. Reject this case until its transitive dependencies can be retained
safely. Test indirect packaging through generic functions, records, and
capturing closures as well as direct introduction; preserve ordinary pure
packages and valid closures that encapsulate hidden types without losing
region dependencies. This conservative fix can precede the richer descriptive
cell/region interface; neither part grants reflection permission to read a
cell or invoke a captured function.

## Answer (safety half)

The immediate requirement is met: packaging that would drop a region is
refused rather than dropped silently.

`Solve::witness` checked only that the witness mentioned no unification
variable minted inside the term. A region is not one of those, so a value
whose type carries a region packaged cleanly and the region went with it. The
rule now walks the resolved witness for a mutable cell anywhere inside it —
directly, in a field, in an element, under an arrow, inside another hidden
type — and refuses with `hidden-region`, saying that a cell's region cannot
be hidden and what to do instead: read the cell and package what it holds, or
keep the value in the scope its region belongs to. Packaging what a cell
holds is unaffected, which is the ordinary case.

Regression: `a_package_will_not_hide_a_cell_region` in
`tests/src/inference.rs` packages a cell directly, inside a record, inside an
array, and under a bare hidden type, and refuses all four; it then packages a
cell's contents and two ordinary values and accepts those. The diagnostic has
a fixture and a golden beside the other inference diagnostics.

Still open in this ticket, and why it is separate: `reification::Node` has no
region node and no cell node, so a type containing `mut` cannot be described
at all, and presence dependencies are tracked under
[13](13-conditional-presence-in-mirrors.md). Conservative rejection precedes
the descriptive work, which is the order the handoff asks for.
