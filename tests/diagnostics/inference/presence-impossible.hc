let bad : { x when 'x: Nat, y when 'y: Nat } -> Nat where 'x != 'y =
  fn value => match value with | { x, y } => x end
