bundle core

type Result = fn err ok => | Ok ok | Err err

type ~Id = fn a => a

let a : Id = ()
