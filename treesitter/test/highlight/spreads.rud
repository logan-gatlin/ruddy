-- The `..` is one operator wherever it spreads or skips: a struct literal's
-- spread, an array literal's, and an array pattern's rest.

let a = { x: 1, ..c }
--              ^^ @operator
--                ^ @variable
let b = [..a, 1]
--       ^^ @operator
let f = fn v => match v with
  | [x, ..rest] => x
--      ^^ @operator
--        ^^^^ @variable
end
