# Whole-file filesystem operations

`std::fs` exposes the `std::FileSystem` capability. The Node runtime
provides its default handler for `main` and root host exports using async foreign functions. Local handlers can
replace it, including in libraries and tests. Suspension is an implementation
detail: ordinary calls remain sequential and require no `await` or `Async`
effect. Dependency libraries retain the capability for their callers to handle.
An alias such as `let read_file = std::fs::read_text` in a Node root library is
callable by JavaScript without passing handlers; its host call may return a Promise.

```text
let main = fn _ => match std::fs::read_text "README.md" with
  | #Some text => std::console::print text
  | #Error error => std::console::print_error error.message
end
```

Every function below has the `!FileSystem` effect. These are interface signatures;
the definitions live in [`std/fs.hc`](../std/fs.hc).

```text
read_bytes: String -> Result [Nat8] Error
write_bytes: String -> [Nat8] -> Result () Error
append_bytes: String -> [Nat8] -> Result () Error
read_text: String -> Result String Error
write_text: String -> String -> Result () Error
append_text: String -> String -> Result () Error
exists: String -> Result Boolean Error
read_dir: String -> Result [DirEntry] Error
metadata: String -> Result Metadata Error
symlink_metadata: String -> Result Metadata Error
create_dir: String -> Result () Error
create_dir_all: String -> Result () Error
remove_file: String -> Result () Error
remove_dir: String -> Result () Error
rename: String -> String -> Result () Error
copy_file: String -> String -> Result () Error

type Error = { kind: ErrorKind, path: String, message: String }
type ErrorKind =
  | #NotFound | #PermissionDenied | #AlreadyExists
  | #NotDirectory | #IsDirectory | #DirectoryNotEmpty
  | #InvalidPath | #InvalidEncoding | #Unsupported | #Other

type FileKind = #File | #Directory | #Symlink | #Other
type Metadata = { kind: FileKind, size: Nat }
type DirEntry = { name: String, kind: FileKind }
```

Writes and appends take the path first and contents second. Binary contents
are immutable arrays of `Nat8` values (`0n8` through `255n8`). Rename and copy take
the source first and destination second. The underlying effect operations
take named-field structs for these multi-argument requests:

```text
write_bytes: { path: String, bytes: [Nat8] } -> Result () fs::Error
append_bytes: { path: String, bytes: [Nat8] } -> Result () fs::Error
write_text: { path: String, text: String } -> Result () fs::Error
append_text: { path: String, text: String } -> Result () fs::Error
rename: { source: String, destination: String } -> Result () fs::Error
copy_file: { source: String, destination: String } -> Result () fs::Error
```

For example, write a file containing a NUL byte, a high-bit byte, and `255`:

```hc
let main = fn _ => match std::fs::write_bytes "data.bin" [0n8, 128n8, 255n8] with
  | #Some _ => ()
  | #Error error => std::console::print_error error.message
end
```

Local `!FileSystem` handlers must cover the three binary operations as well as
the text and directory operations. Binary write and append handlers receive
`{ path, bytes }`; binary read handlers return `#Some bytes` or `#Error error`.

## Paths, contents, and errors

Paths are strings interpreted by the host filesystem. Relative paths use the
process working directory. NULs and unpaired Unicode surrogates return
`#InvalidPath`; directory listings containing names that cannot be represented
as UTF-8 also return `#InvalidPath`.

Text is UTF-8. Reads reject malformed bytes with `#InvalidEncoding` and preserve
the BOM, if present. Writes and appends reject unpaired surrogates before
opening or modifying the destination. Binary operations preserve every byte
without decoding or encoding, including NULs, malformed UTF-8, and BOMs.
Empty files read as `[]`. Files are read entirely into memory; binary writes
and appends leave their input arrays unchanged.

`write_text` and `write_bytes` create a missing file or truncate an existing
file. `append_text` and `append_bytes` create a missing file or append to an
existing file. Empty writes still create or truncate the destination; empty
appends create a missing file and otherwise leave its contents unchanged.
None create parent directories, and all follow symlinks. Reading a directory
as either text or bytes returns `#IsDirectory`.

`exists` follows symlinks. It returns `#Some false` for a missing path or dangling
symlink, and `#Some true` for an existing target, including a directory.
Permission errors and other failures remain `#Error`; a non-directory path
component returns `#NotDirectory`. An existence check does not guarantee the
outcome of a subsequent operation.

Match `error.kind` for stable error categories. `error.message` is diagnostic
text and may vary by platform. `error.path` is the path reported by the host,
or the requested path when the host does not provide one. For rename and copy,
the fallback is the source; the message may contain destination details.
Unclassified host errors, including symlink loops, return `#Other`.

## Directories, metadata, and mutations

`read_dir` lists immediate children in unspecified order. Each entry has a
basename, and its kind describes the entry without following symlinks.
`metadata` follows symlinks; `symlink_metadata` inspects the link itself.
`Metadata.size` is the host-reported byte size, meaningful as content length
for regular files. Other file kinds retain the host's size semantics.

`create_dir` creates one directory and fails if it already exists.
`create_dir_all` creates missing parents and succeeds if the requested directory
already exists. Neither replaces an existing file.

`remove_file` unlinks a file or symlink, leaving a symlink's target intact.
`remove_dir` removes only an empty directory. Missing removal targets return
`#NotFound`.

Rename and copy use the host's replacement rules and may replace existing
destination files. Rename can fail across filesystems. Copy copies file contents
without promising metadata preservation; it follows source and destination
symlinks according to the host's file-copy behavior. Writes and copies provide
no atomicity or durability guarantee; a failed operation may leave a partial
destination. Rename, directory operations, and concurrent access retain host
filesystem semantics.

The adapter uses Node's [promise-based filesystem functions](https://nodejs.org/api/fs.html#promises-api)
and [fatal UTF-8 decoding](https://nodejs.org/api/util.html#new-textdecoderencoding-options).
Streams, watchers, handles, and recursive deletion are outside this
module's current interface.
