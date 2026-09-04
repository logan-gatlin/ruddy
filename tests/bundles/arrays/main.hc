let len = std::array::len
let push = std::array::push
let prepend = std::array::prepend
let concat = std::array::concat
let get_or = fn arr i fallback => match std::array::get arr i with
  | #Some value => value
  | #None => fallback
end
let set_or = fn arr i value => match std::array::set arr i value with
  | #Some updated => updated
  | #None => arr
end
let slice_or = fn arr from to => match std::array::slice arr from to with
  | #Some part => part
  | #None => []
end
let slice_ok = fn arr from to => match std::array::slice arr from to with
  | #Some _ => true
  | #None => false
end
let pop_last = fn arr fallback => match std::array::pop arr with
  | #Some (last, _) => last
  | #None => fallback
end
let pop_rest = fn arr => match std::array::pop arr with
  | #Some (_, rest) => rest
  | #None => arr
end
let pop_ok = fn arr => match std::array::pop arr with
  | #Some _ => true
  | #None => false
end
let empty = []
let count = fn arr => match arr with
  | [] => 0n
  | [_, ..rest] => std::nat::add 1n (count rest)
end
let last_or = fn arr fallback => match arr with
  | [.., last] => last
  | [] => fallback
end
let ends = fn arr => match arr with
  | [first, .., last] => [first, last]
  | [..] => []
end
let init_of = fn arr => match arr with
  | [..init, _] => init
  | [] => []
end
let middle = fn arr => match arr with
  | [_, ..inner, _] => inner
  | [..] => []
end
let describe = fn arr => match arr with
  | [] => "empty"
  | [0n] => "zero"
  | [0n, 0n, ..] => "zeros"
  | [x] => "one"
  | [_, _, ..] => "many"
end
let wrap = fn arr => [..arr, ..arr]
let around = fn arr x => [x, ..arr, x]
let copy = fn arr => [..arr]
let none = fn arr => [..arr, ..[]]
