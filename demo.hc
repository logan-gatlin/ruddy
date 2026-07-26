let id = fn x => x

let compose = fn f g x => f (g x)

let point = { x: origin, y: shift base }

type Pair = { first: A, second: B }

type Lambda = fn t => { value: t }

type Pairs = List Pair

let after = id point

let count = 42

let scaled = compose id id 4096

let sizes = { small: 0, large: 340282366920938463463374607431768211455 }

let bad = @

let malformed = 12abc

type = missingName
