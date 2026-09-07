type Result 'some 'error = #Some 'some | #Error 'error

module fs =
  type ErrorKind =
    | #NotFound
    | #PermissionDenied
    | #AlreadyExists
    | #NotDirectory
    | #IsDirectory
    | #DirectoryNotEmpty
    | #InvalidPath
    | #InvalidEncoding
    | #Unsupported
    | #Other

  type Error = { kind: ErrorKind, path: String, message: String }

  type FileKind = #File | #Directory | #Symlink | #Other

  type Metadata = { kind: FileKind, size: Nat }
  type DirEntry = { name: String, kind: FileKind }
end

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

effect Console = { write: String -> (), write_error: String -> () }
effect Process = { exit: Nat -> | }
extern write : fn(String) -> () = "text => { process.stdout.write(text); }"
extern write_error : fn(String) -> () = "text => { process.stderr.write(text); }"
@async
extern fs_read_text: String -> Result String fs::Error = "$fs.read_text"
@async
extern fs_write_text: { path: String, text: String } -> Result () fs::Error = "$fs.write_text"
@async
extern fs_append_text: { path: String, text: String } -> Result () fs::Error = "$fs.append_text"
@async
extern fs_exists: String -> Result Boolean fs::Error = "$fs.exists"
@async
extern fs_read_dir: String -> Result FsEntries fs::Error = "$fs.read_dir"
@async
extern fs_metadata: String -> Result fs::Metadata fs::Error = "$fs.metadata"
@async
extern fs_symlink_metadata: String -> Result fs::Metadata fs::Error = "$fs.symlink_metadata"
@async
extern fs_create_dir: String -> Result () fs::Error = "$fs.create_dir"
@async
extern fs_create_dir_all: String -> Result () fs::Error = "$fs.create_dir_all"
@async
extern fs_remove_file: String -> Result () fs::Error = "$fs.remove_file"
@async
extern fs_remove_dir: String -> Result () fs::Error = "$fs.remove_dir"
@async
extern fs_rename: { source: String, destination: String } -> Result () fs::Error = "$fs.rename"
@async
extern fs_copy_file: { source: String, destination: String } -> Result () fs::Error = "$fs.copy_file"

-- Arrays are private runtime values and cannot cross ordinary externs. The
-- directory adapter returns a concrete list, converted inside Ruddy instead.
type FsEntries = #Cons (fs::DirEntry, FsEntries) | #None
extern array_push: ['a] -> 'a -> ['a] = "$arrayPush"
let collect_entries: FsEntries -> [fs::DirEntry] -> [fs::DirEntry] = fn entries values => match entries with
  | #Cons (entry, rest) => collect_entries rest (array_push values entry)
  | #None => values
end
let read_dir = fn path => match fs_read_dir path with
  | #Some entries => #Some (collect_entries entries [])
  | #Error error => #Error error
end

@async
extern host_exit: Nat -> | = "$nodeExit"

