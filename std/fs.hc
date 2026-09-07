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

let read_bytes = !FileSystem.read_bytes

let write_bytes = fn path bytes =>
  !FileSystem.write_bytes { path: path, bytes: bytes }

let append_bytes = fn path bytes =>
  !FileSystem.append_bytes { path: path, bytes: bytes }

let read_text = !FileSystem.read_text

let write_text = fn path text =>
  !FileSystem.write_text { path: path, text: text }

let append_text = fn path text =>
  !FileSystem.append_text { path: path, text: text }

let exists = !FileSystem.exists

let read_dir = !FileSystem.read_dir

let metadata = !FileSystem.metadata

let symlink_metadata = !FileSystem.symlink_metadata

let create_dir = !FileSystem.create_dir

let create_dir_all = !FileSystem.create_dir_all

let remove_file = !FileSystem.remove_file

let remove_dir = !FileSystem.remove_dir

let rename = fn source destination =>
  !FileSystem.rename { source: source, destination: destination }

let copy_file = fn source destination =>
  !FileSystem.copy_file { source: source, destination: destination }
