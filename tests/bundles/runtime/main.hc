module Math

extern next : Nat -> Nat = host.counter.next
extern add : fn(Nat, Nat) -> Nat = host.counter.add
extern nullary : fn() -> Nat = host.counter.nullary
extern curried_add : Nat -> Nat -> Nat = host.curriedAdd
extern apply_pair : fn(fn(Nat, Nat) -> Nat, Nat, Nat) -> Nat = host.applyPair
extern make_adder : fn(Nat) -> fn(Nat, Nat) -> Nat = host.makeAdder

effect Tick = Nat -> Nat
extern tick : fn(Nat) -> Nat + !Tick = host.tick
extern run_tick : fn(fn(Nat) -> Nat + !Tick, Nat) -> Nat + !Tick = host.runTick

let answer = Math::answer
let identity = Math::identity
let apply = fn f => fn x => f x
let captured = fn x => fn y => x + y
let added = add 20n 2n
let add_twenty = add 20n
let nullary_answer = nullary ()
let curried_answer = curried_add 20n 22n
let callback_answer = apply_pair (fn x => fn _ => x) 42n 0n
let nested_result = make_adder 2n 20n 20n
let ticked = handle tick 42n with
  | !Tick value => value
end
let callback_ticked = handle run_tick (fn value => !Tick value) 42n with
  | !Tick value => value
end
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
