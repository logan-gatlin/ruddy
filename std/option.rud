let map : ('a -> 'b + ..'effects) -> Option 'a -> Option 'b + ..'effects =
  fn function option => match option with
  | #Some value => #Some (function value)
  | #None => #None
  end

let and_then : ('a -> Option 'b + ..'effects) -> Option 'a -> Option 'b + ..'effects =
  fn function option => match option with
  | #Some value => function value
  | #None => #None
  end

let unwrap_or : 'a -> Option 'a -> 'a = fn fallback option => match option with
  | #Some value => value
  | #None => fallback
end

let or_else : (() -> Option 'a + ..'effects) -> Option 'a -> Option 'a + ..'effects =
  fn fallback option => match option with
  | #Some _ => option
  | #None => fallback ()
  end

let filter : ('a -> Boolean + ..'effects) -> Option 'a -> Option 'a + ..'effects =
  fn predicate option => match option with
  | #Some value => if predicate value then option else #None end
  | #None => #None
  end

let flatten : Option (Option 'a) -> Option 'a = fn option => match option with
  | #Some inner => inner
  | #None => #None
end

let is_some : Option 'a -> Boolean = fn option => match option with
  | #Some _ => true
  | #None => false
end

let is_none : Option 'a -> Boolean = fn option => match option with
  | #Some _ => false
  | #None => true
end

let some_or : 'a -> (#Some 'a | ..'rest) -> 'a = fn fallback value => match value with
  | #Some some => some
  | _ => fallback
end

let is_some_case : (#Some 'a | ..'rest) -> Boolean = fn value => match value with
  | #Some _ => true
  | _ => false
end
