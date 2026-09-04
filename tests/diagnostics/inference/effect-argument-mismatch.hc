effect Ask 'a = { get: () -> 'a }
let bad = fn _ => do let n : Nat = !Ask.get () let s : String = !Ask.get () end
