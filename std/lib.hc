type Never = |

type Option 't = #Some 't | #None
type Maybe 't = #Nil | ..'t

type Result 'some 'error = #Some 'some | #Error 'error
type Fallible 'error 'rest = #Error 'error | ..'rest

let map_some = fn f maybe => match maybe with
| #Some s => #Some (f s)
| _ => maybe
end

type List 'a = #Cons ('a, List 'a) | #None

type Ordering = #Less | #Equal | #Greater

module str
module nat
module int
module real
module boolean
module option
module result
module list
module array
module function
module tuple
module ordering
