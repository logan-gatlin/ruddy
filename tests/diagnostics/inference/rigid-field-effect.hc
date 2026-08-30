effect Log = { write: Nat -> () }
let greet : () -> Nat + !Log = fn _ => let _ = !Log.write 1n in 0n
let bad : () -> Nat + ..'effects = fn _ => greet ()
