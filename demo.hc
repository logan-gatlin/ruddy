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

let open : { small: Nat, ..extra } -> Nat = fn s => s.small

let opt : { label?: Nat, value: Nat, .. } -> Nat = fn r => r.value

type Option T = `Some T | `None

let some = `Some 1

let nothing : Option Nat = `None

let wrap = fn x => `Some x

type Tree a = `Leaf | `Branch { value: a, kids: Tree a }

let leaf : Tree Nat = `Leaf

type Fallible r = `Err Nat | ..r

let handled : Fallible (`Ok Nat) -> Nat = fn t => 0

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

let bad = @

let malformed = 12abc

type = missingName
