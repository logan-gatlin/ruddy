# 20 Validate debugger integer target configurations

Status: resolved
Type: task
Priority: P1

Review baseline: `cd906cf`. This is a newly identified debugger miscompilation.

The [spec's primitive contracts](../spec.md#portable-primitive-contracts) require
exact results for valid in-domain integers and diagnose literals when their
target domain is bound. JavaScript currently supports target integer domains
of 32 or 53 bits; fixed-width `Nat64`/`Int64` are separate supported types.

## Reproduction and impact

In the debugger, select target `js` and integer precision `64-bit`, then compile:

```ruddy
let distinct = match 9007199254740993n with
| 9007199254740992n => false
| _ => true
end
```

Compilation reports no diagnostics and the generated JavaScript evaluates
`distinct` to `false`: both literals become the same JavaScript `number`.
Expected: reject this unsupported target/domain combination before executable
JavaScript is generated. The declared 64-bit domain makes both literals valid,
so this cannot be dismissed as undefined out-of-domain arithmetic.

## Work

The debugger constructs a public `Build` directly in
[snapshot.rs](../../../debug/src/snapshot.rs), bypassing the manifest checks in
[cli/src/lib.rs](../../../cli/src/lib.rs). Its `and_then(...).unwrap_or_default()`
also silently replaces an invalid requested precision with the default.

- Put target/domain validation behind a shared validated construction path or
  equivalent central check used by both CLI and debugger. Prevent alternate
  compilation entry points from bypassing the same invariant.
- Return a useful configuration diagnostic for unsupported combinations and
  unknown integer precisions. An omitted precision may use the default; an
  explicitly invalid one must not silently do so.
- Preserve valid JavaScript 32/53-bit builds, artifact/interpreter 64-bit
  configurations, and explicit fixed-width 64-bit types on JavaScript.

## Acceptance

Add debugger request coverage for JavaScript plus 64 bits and an unknown
precision such as 17. Neither request produces executable code under an
unannounced fallback. Cover valid JavaScript 32/53-bit requests and a valid
64-bit artifact request through the shared validation path. Keep the literal
distinction above as a regression case. Run Rust tests only through `just test`.

## Answer

`ruddy_cli::Build::resolve(target, platform, integers)` is the one validated
construction. It takes the default when no precision is written, reports
`manifest-invalid` for a precision the compiler does not bind, and reports the
same for JavaScript with 64-bit integers. `Manifest::build` calls it, so the
command line is unchanged, and `debug/src/snapshot.rs` calls it instead of
building a `Build` itself.

A refused configuration compiles nothing: the diagnostic is recorded and the
bundle is never loaded, so every phase over source reports that it did not
run and no JavaScript is generated. The debugger no longer replaces an
invalid precision with the default.

Regression: `a_request_the_compiler_cannot_bind_compiles_nothing` in
`tests/src/snapshot.rs` sends the ticket's two-literal program at JavaScript
with 64 bits and with 17 bits. Each reports exactly `manifest-invalid`, the
JavaScript phase is skipped, and no phase over source reports a result. The
supported combinations still compile: JavaScript at 32, 53, and the default,
and an artifact at 64, where the same two literals compile without complaint.
