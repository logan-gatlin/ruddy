---
doc: true
layout: std.njk
stdReference: true
---

# [std](bundle.md)::test

Assertions and local effect handlers for tests and mocks.

## Types

### FileSystemCall

```ruddy
type FileSystemCall =
  | #ReadBytes String
  | #WriteBytes { path: String, bytes: [Nat8] }
  | #AppendBytes { path: String, bytes: [Nat8] }
  | #ReadText String
  | #WriteText { path: String, text: String }
  | #AppendText { path: String, text: String }
  | #Exists String
  | #ReadDir String
  | #Metadata String
  | #SymlinkMetadata String
  | #CreateDir String
  | #CreateDirAll String
  | #RemoveFile String
  | #RemoveDir String
  | #Rename { source: String, destination: String }
  | #CopyFile { source: String, destination: String }
```

Recorded filesystem requests in invocation order.

### FileSystemResponses

```ruddy
type FileSystemResponses = {
  read_bytes: Option (String -> Result [Nat8] fs::Error),
  write_bytes: Option ({ path: String, bytes: [Nat8] } -> Result () fs::Error),
  append_bytes: Option ({ path: String, bytes: [Nat8] } -> Result () fs::Error),
  read_text: Option (String -> Result String fs::Error),
  write_text: Option ({ path: String, text: String } -> Result () fs::Error),
  append_text: Option ({ path: String, text: String } -> Result () fs::Error),
  exists: Option (String -> Result Bool fs::Error),
  read_dir: Option (String -> Result [fs::DirEntry] fs::Error),
  metadata: Option (String -> Result fs::Metadata fs::Error),
  symlink_metadata: Option (String -> Result fs::Metadata fs::Error),
  create_dir: Option (String -> Result () fs::Error),
  create_dir_all: Option (String -> Result () fs::Error),
  remove_file: Option (String -> Result () fs::Error),
  remove_dir: Option (String -> Result () fs::Error),
  rename: Option ({ source: String, destination: String } -> Result () fs::Error),
  copy_file: Option ({ source: String, destination: String } -> Result () fs::Error),
}
```

Optional pure response functions for mock_filesystem. None marks an unexpected operation.

### HttpCall

```ruddy
type HttpCall =
  #Request http::Request
```

Recorded http requests in invocation order.

### HttpResponses

```ruddy
type HttpResponses = {
  request: Option (http::Request -> Result http::Response http::Error),
}
```

Optional pure response functions for mock_http. None marks an unexpected operation.

### PathCall

```ruddy
type PathCall =
  | #Resolve [String]
  | #Relative { from: String, to: String }
```

Recorded path requests in invocation order.

### PathResponses

```ruddy
type PathResponses = {
  resolve: Option ([String] -> Result String path::Error),
  relative: Option ({ from: String, to: String } -> Result String path::Error),
}
```

Optional pure response functions for mock_path. None marks an unexpected operation.

### ProcessCall

```ruddy
type ProcessCall =
  | #Args ()
  | #Env String
  | #Cwd ()
```

Recorded process requests in invocation order.

### ProcessResponses

```ruddy
type ProcessResponses = {
  args: Option (() -> [String]),
  env: Option (String -> Result (Option String) process::Error),
  cwd: Option (() -> Result String process::Error),
}
```

Optional pure response functions for mock_process. None marks an unexpected operation.

## Effects

### Assert

```ruddy
effect Assert = String -> ()
```

Reports an assertion failure. A handler may return unit to continue, or halt the computation.

## Values

### assert

```ruddy
let assert: Bool -> String -> () + !Assert
```

Reports the message when the condition is false.

### fail

```ruddy
let fail: String -> () + !Assert
```

Reports an unconditional assertion failure; a handler may resume after it.

### filesystem_defaults

```ruddy
let filesystem_defaults: FileSystemResponses
```

No operations are expected. Override fields with Some response functions using a record spread.

### http_defaults

```ruddy
let http_defaults: HttpResponses
```

No operations are expected. Override fields with Some response functions using a record spread.

### mock_filesystem

```ruddy
let mock_filesystem: FileSystemResponses -> _
```

Handles FileSystem using supplied responses and returns {value, calls}. Unexpected operations assert; if the assertion resumes, they return an error (or empty arguments).

### mock_http

```ruddy
let mock_http: HttpResponses -> _
```

Handles Http using supplied responses and returns {value, calls}. Unexpected operations assert; if the assertion resumes, they return an error (or empty arguments).

### mock_path

```ruddy
let mock_path: PathResponses -> _
```

Handles Path using supplied responses and returns {value, calls}. Unexpected operations assert; if the assertion resumes, they return an error (or empty arguments).

### mock_process

```ruddy
let mock_process: ProcessResponses -> _
```

Handles Process using supplied responses and returns {value, calls}. Unexpected operations assert; if the assertion resumes, they return an error (or empty arguments).

### path_defaults

```ruddy
let path_defaults: PathResponses
```

No operations are expected. Override fields with Some response functions using a record spread.

### process_defaults

```ruddy
let process_defaults: ProcessResponses
```

No operations are expected. Override fields with Some response functions using a record spread.

<!-- Generated by ruddy doc for std. -->
