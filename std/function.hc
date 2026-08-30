let identity : 'a -> 'a = fn value => value
let constant : 'a -> 'b -> 'a = fn value _ => value

let apply : ('a -> 'b + ..'effects) -> 'a -> 'b + ..'effects =
  fn function value => function value

let pipe : 'a -> ('a -> 'b + ..'effects) -> 'b + ..'effects =
  fn value function => function value

let compose : ('b -> 'c + ..'effects) -> ('a -> 'b + ..'effects) -> 'a -> 'c + ..'effects =
  fn outer inner value => outer (inner value)

let flip : ('a -> 'b -> 'c + ..'effects) -> 'b -> 'a -> 'c + ..'effects =
  fn function second first => function first second

let curry : (('a, 'b) -> 'c + ..'effects) -> 'a -> 'b -> 'c + ..'effects =
  fn function first second => function (first, second)

let uncurry : ('a -> 'b -> 'c + ..'effects) -> ('a, 'b) -> 'c + ..'effects =
  fn function values => function (values.0) (values.1)

