let id = fn x => x

let compose = fn f g x => f (g x)

let point = { x: origin, y: shift base }

type Pair A B = { first: A, second: B }

type Boxed = { value: Nat }

type list = { val: Nat, next: list }

let rest : list -> list = fn l => l.next

type Pairs = Pair Nat Nat -> Pair Nat Nat

let swap : Pair Nat Boxed -> Pair Boxed Nat = fn p => { first: p.second, second: p.first }

type Stack a = { top: a, rest: Stack a }

let peek : Stack Nat -> Nat = fn s => s.top

type Tagged r = { tag: Nat, ..r }

let tag : Tagged { note: Nat } -> Nat = fn t => t.tag

let tagged = tag { tag: 7, note: 8 }

let boxed : Tagged Nat -> Nat = fn b => b.tag

let after = id point

let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x

let n = fst { x: 1, y: 2 }

type Endo = Nat -> Nat

let count = 42

let scaled = compose id id 4096

let sizes = { small: 0, large: 340282366920938463463374607431768211455 }

let getx = fn p => p.x

let dot = getx { x: 3, y: 4 }

let open : { small: Nat, ..'extra } -> Nat = fn s => s.small

let opt : { label when 'a: Nat, value: Nat, .. } -> Nat = fn r => r.value

let either = fn v => match v with
  | {x} => {}
  | {y} => {}
end

let both : { x when 'a: 'c, y when 'b: 'd } -> {} where 'a = 'b
  = fn v =>
    match v with
      | {x, y} => {}
      | {} => {}
    end

type Option T = #Some T | #None

let some = #Some 1

let nothing : Option Nat = #None

let wrap = fn x => #Some x

type Tree a = #Leaf | #Branch { value: a, kids: Tree a }

let leaf : Tree Nat = #Leaf

type Fallible r = #Err Nat | ..r

let handled : Fallible (#Ok Nat) -> Nat = fn t => 0

let identical : 'a -> 'a = fn x => x

let firstly : { x: 'a, .. } -> 'a = fn p => p.x

let widened : (#Err Nat | ..'r) -> Nat = fn t => 0

let anonymous : { .. } -> Nat = fn s => 0

let unnamed : { flag when _: Nat, .. } -> Nat = fn s => 0

let holed : _ -> Nat = fn x => 0

let narrowed : Nat -> Nat = fn x => x

type Nest = { kids: Nest }

let depth : { kids: Nest, ..'r } -> Nat = fn n => depth n.kids

let repeat = fn x => repeat x

let ping = fn n => pong n

let pong = fn n => ping n

let forwarded = identity 7

let identity = fn x => x

let named =
  let half = { v: 21 } in
  { doubled: half.v, original: half }

let twice =
  let same = fn x => x in
  { a: same 1, b: same {} }

let selfnaming =
  let step = fn n => step n in
  step

let ascribed =
  let total : Nat = 4 in
  total

let shadowing =
  let count = { v: 1 } in
  count

let _ = id 7

let konst = fn x _ => x

let unwrap = fn opt => match opt with
  | #Some x => x
  | _ => 0
  end

effect Log = write : Nat -> ()

effect IO = print : Nat -> ()

effect Console = !Log + !IO

let greet : () -> Nat + !Log = fn _ =>
  let _ = !Log.write 1 in
  0

let quiet : () -> Nat = fn _ =>
  handle greet () with
    | !Log.write s => ()
    | return x => x
  end

let loud : () -> Nat + !Console = fn _ =>
  handle greet () with
    | !Log.write s => !IO.print s
  end

effect Fail = oops : () -> Nat

let recover : () -> Nat = fn _ =>
  handle !Fail.oops () with
    | !Fail.oops _ => raise 0
  end

let runs = fn g => handle g () with | !Log.write s => () end

let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g n => g n

type Logger = Nat -> Nat + !Log

type Runner e = (Nat -> Nat + ..e) -> Nat + ..e

let logged : Runner (!Log) -> Nat = fn r => 0

let consoled : Runner (!Log + !IO) -> Nat = fn r => 0

let bad = @

let malformed = 12abc

type = missingName
