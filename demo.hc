let id = fn x => x

let compose = fn f g x => f (g x)

let point = { x: origin, y: shift base }

type Pair = { first: A, second: B }

type Boxed = { value: Nat }

type list = { val: Nat, next: list }

let rest : list -> list = fn l => l.next

type Pairs = Pair -> Pair

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

let bad = @

let malformed = 12abc

type = missingName
