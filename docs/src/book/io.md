---
doc: true
bookNavigation:
  previous:
    path: "/book/state.html"
    title: "9. Mutable state and regions"
  next:
    path: "/book/external-data.html"
    title: "11. Structured data and network requests"
---

# 10. Input, output, and failure

A level loader obtains data from an environment that may not behave as expected.
[Effects](effects.md) describe those interactions, while [Result](../std/result.md) describes whether an operation produced a usable result.
The two solve different problems: an operation can be effectful and still return a failure as ordinary data.

## Reading a file

The filesystem API returns either text or a structured error.
This function reports a successful read or the error message:

```ruddy
let show_file = fn path => match std::fs::read_text path with
| #Some text => println text
| #Error error => eprintln error.message
end
```

Both branches produce unit and perform output, but use different streams.
The filesystem error also has a kind and path, allowing a caller to choose a recovery policy without parsing the message.
The [Filesystem](../std/fs.md) reference describes those cases and the available operations.

This function reports failure but does not choose a nonzero process exit status.
An executable may make that decision at its outer boundary, as the [worked program](report.md) does.
A reusable library generally returns information its caller can interpret.

## Process inputs

[Process](../std/process.md) supplies arguments, environment variables, and the working directory.
Arguments exclude the Node executable and script path.
An optional argument can be selected through array matching:

```ruddy
let input_path = fn _ => match std::process::args () with
| [] => "level.json"
| [path] => path
| _ => "level.json"
end
```

This example deliberately defaults for both zero and several arguments.
A real command may instead reject the latter case, which is a policy decision rather than an array-pattern rule.
The worked program makes that distinction.

An environment lookup returns a result containing an option.
`#Some #None` means the lookup succeeded but the name is absent.
`#Some (#Some "")` means the variable exists with an empty value.
`#Error error` means the lookup itself failed.
Collapsing these cases would remove information a caller may need.

## Paths and ambient behavior

[Path](../std/path.md) supplies path operations for Node.
Joining path components expresses platform-aware construction without depending on literal separators:

```ruddy
let level_path = std::path::join ["levels", "forest.json"]
```

Lexical operations such as joining do not inspect the filesystem.
Resolving relative paths can depend on the working directory, which is an environmental dependency.
The platform-specific [POSIX](../std/path.posix.md) and [Windows](../std/path.windows.md) modules provide explicit conventions when the host default is not appropriate.

## Replacing the environment

A local process handler can make an operation repeatable without reading the real process state:

```ruddy
let sample_arguments = fn _ => handle std::process::args () with
| std::process::!Process.args _ => ["test-level.json"]
| std::process::!Process.env _ => #Some #None
| std::process::!Process.cwd _ => #Some "/example"
end
```

The complete interface is supplied even though the handled computation only requests arguments.
The result is `["test-level.json"]` independently of the actual process arguments.
This technique is useful for exercising application decisions while reserving separate checks for the host adapter itself.

## Summary and exercises

Environment-dependent operations belong at explicit boundaries.
Their returned data should preserve distinctions among success, absence, and failure until the responsible caller chooses a policy.

1. Change `input_path` to return a result that rejects more than one argument.
2. Give a policy for a missing environment variable that differs from a failed lookup.
3. Use the process handler to exercise the invalid-argument branch without changing shell state.

[Selected answers](answers.md#input-and-failure) explain the result shape.

