effect Ask 'a = { get: () -> 'a }
let bad = fn _ => let n : Nat = !Ask.get () in let s : String = !Ask.get () in ()
