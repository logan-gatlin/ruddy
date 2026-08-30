let bad = fn captured =>
  let identity : 'a -> 'a = captured in
  0n
