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
