module Math

extern next : Nat -> Nat = host.counter.next

let answer = Math::answer
let identity = Math::identity
let apply = fn f => fn x => f x
let captured = fn x => fn y => x + y
let record = { answer: answer, ready: true }
let tagged = #Ready answer
let read_tag = fn value => match value with
  | #Ready payload => payload
  | _ => 0n
end
let classify_record = fn value => match value with
  | { answer, ready } => answer
  | _ => 0n
end

effect Bump = Real -> Real
let bump = fn value => handle !Bump value with
  | !Bump current => current + 1.0
end
