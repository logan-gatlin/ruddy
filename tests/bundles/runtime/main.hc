module Math

extern next : Nat -> Nat = "host.counter.next.bind(host.counter)"
extern add : fn(Nat, Nat) -> Nat = "host.counter.add.bind(host.counter)"
extern nullary : fn() -> Nat = "host.counter.nullary.bind(host.counter)"
extern curried_add : Nat -> Nat -> Nat = "host.curriedAdd"
extern apply_pair : fn(fn(Nat, Nat) -> Nat, Nat, Nat) -> Nat = "host.applyPair"
extern make_adder : fn(Nat) -> fn(Nat, Nat) -> Nat = "host.makeAdder"

effect Tick = Nat -> Nat
extern tick : fn(Nat) -> Nat + !Tick = "host.tick"
extern run_tick : fn(fn(Nat) -> Nat + !Tick, Nat) -> Nat + !Tick = "host.runTick"
extern run_returned_tick : fn(fn(Nat) -> (Nat -> Nat + !Tick), Nat, Nat) -> Nat + !Tick = "host.runReturnedTick"

effect Needed = () -> Nat
effect Spare = Nat -> Nat
extern invoke_conditional_shared : fn(fn(()) -> Nat + !Needed (when 'needed) + ..'effects) -> Nat + !Needed + ..'effects = "host.invokeConditionalShared"
let run_conditional_shared : () -> Nat + !Needed + ..'effects =
  fn _ => let needed = !Needed () in invoke_conditional_shared (fn _ => needed)

type Loop = () -> Loop
extern host_loop : Loop = "host.loop.bind(host)"
extern run_loop : fn(Loop) -> Nat = "host.runLoop"

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
let returned_callback_ticked = handle run_returned_tick (fn a => fn b => !Tick a) 42n 22n with
  | !Tick value => value
end
let conditional_shared_result = handle run_conditional_shared ()
  with | !Needed _ => 42n end
let host_looped = run_loop host_loop
let ruddy_loop : Loop = fn _ => ruddy_loop
let ruddy_looped = run_loop ruddy_loop
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
