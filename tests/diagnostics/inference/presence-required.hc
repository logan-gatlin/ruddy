let one : { x when 'x: Nat, y when 'y: Nat } -> Nat where 'x != 'y =
  fn value => match value with | { x } => x | { y } => y end
let bad = one {}
