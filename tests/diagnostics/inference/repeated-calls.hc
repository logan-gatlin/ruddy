let identity = fn value => value
let first = identity 1n
let second = identity {}
let bad : Nat = second
