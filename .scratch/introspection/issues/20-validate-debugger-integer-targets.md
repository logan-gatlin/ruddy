# 20 Validate debugger integer target configurations

Status: open
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
