effect Log = { write: Nat -> () }
let bad = fn _ => handle !Log.write 1n with | !Log.write n => !Log.write n end
