# All variable kinds in type definitions

Support inferred type, row, effect, region, and presence parameters in type headers.
All free variables must be declared in the header. Reject anonymous variables and inference holes anywhere inside definitions. Preserve local whole-type hide binders.
Presence arguments accept true, false, and variables, not arbitrary formulas.
Definitions support where constraints, including parameters used only in constraints. Infer kinds from bodies and constraints; reject incompatible kinds and undeclared names.
Inherit constraints through aliases, nested types, and recursive composition. Reject impossible definitions, and enforce constraints at applications.
Annotations retain implicit variables and holes, infer presence kind from application arguments, and allow where constraints on forwarded presences.
Preserve structural equality, existing recursive-type rules, shared struct/sum row kind, separate effect kind, and unused parameters defaulting to Type.

Tests: source-level parser/compiler acceptance and rejection, parser/formatter round trips (confirmed by user).
Review baseline: 68d8adb2265b642c2f698fa1d661a6bc5c04bd23.
