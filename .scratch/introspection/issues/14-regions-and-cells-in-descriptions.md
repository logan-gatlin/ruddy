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
