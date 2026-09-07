-- A block's words are keywords, its `return` the one that hands a value
-- back — as `raise` is — and a binding inside it is highlighted as the
-- definition it reads like.

let total = do
-- <- @keyword
--          ^^ @keyword
  let step = fn n => n
--    ^^^^ @function
  let count = 1
--    ^^^^^ @variable
  return step count
--^^^^^^ @keyword.return
end
-- <- @keyword

let quiet = fn _ => handle greet () with
  | return x => x
--  ^^^^^^ @keyword.return
end

let unwrap = fn | #Some value => value | #None => 0n
--  ^^^^^^ @function
--                ^^^^^ @constructor
