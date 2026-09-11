# 04 Shapes, typed views, bindings, and builders

Status: open
Type: task
Blocked by: 03

Spec sections: "Heterogeneous fields without erasing their types", the
generic-derivation algorithm, and the acceptance bullets on package
abstraction, joint conditional presences, and sealed packages.

- `reflect::shape : Mirror 'a -> Shape 'a` with primitive cases carrying typed
  `read`/`make`, `#Array (ArrayView 'a)`, `#Record (RecordView 'a)`,
  `#Sum { cases: [SomeCase 'a] }`, and descriptive-only cases for functions,
  cells, hidden packages, and abstract positions.
- `SomeField`, `RecordView`, `Binding`, `SomeCase`, `ArrayView` as hidden
  packages defined in Ruddy; builders reject missing required fields,
  duplicates, foreign bindings, and invalid joint presence combinations.
- A generic printer/validator written in Ruddy over these views, exercised end
  to end through generated JS.

Seams: generated JS execution, `tests/src/inference.rs` for the typing of the
view packages.
