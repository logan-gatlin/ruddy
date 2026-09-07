type Never = |

effect Console = { write: String -> (), write_error: String -> () }
effect Process = { exit: Nat -> Never }

effect FileSystem = {
  read_text: String -> Result String fs::Error,
  write_text: { path: String, text: String } -> Result () fs::Error,
  append_text: { path: String, text: String } -> Result () fs::Error,
  exists: String -> Result Boolean fs::Error,
  read_dir: String -> Result [fs::DirEntry] fs::Error,
  metadata: String -> Result fs::Metadata fs::Error,
  symlink_metadata: String -> Result fs::Metadata fs::Error,
  create_dir: String -> Result () fs::Error,
  create_dir_all: String -> Result () fs::Error,
  remove_file: String -> Result () fs::Error,
  remove_dir: String -> Result () fs::Error,
  rename: { source: String, destination: String } -> Result () fs::Error,
  copy_file: { source: String, destination: String } -> Result () fs::Error
}

module fs

module console
module process

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
