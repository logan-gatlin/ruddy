effect Log = { write: Nat -> () }
let greet : () -> Nat + !Log = fn _ => do let _ = !Log.write 1n return 0n end
let bad = greet ()
