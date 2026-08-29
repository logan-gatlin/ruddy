type Option 'value = #Some 'value | #None

type Result 'value 'error = #Ok 'value | #Err 'error

let identity = fn value => value

let constant = fn value _ => value

let compose = fn outer inner value => outer (inner value)

let flip = fn function left right => function right left
