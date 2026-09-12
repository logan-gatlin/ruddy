# Reflection-based comparison operators

Agreed through the comparison design interview.

- Provide pure `==`, `!=`, `<`, `<=`, `>`, and `>=` for operands sharing an inferred static type. Keep existing row inference, including `#A == #B` inferring a shared sum.
- Add `order::PartialOrder` with `Less`, `Equal`, `Greater`, and `Unordered`. Preserve `order::Order` for total orderings. Expose reflection-based comparison as an ordinary library API used by the operators.
- Unordered yields false for every operator except `!=`. Functions and unsupported reflection shapes are always unordered, even against themselves. No identity tokens, opt-out, or automatic custom overrides.
- Opened existential types require mirror evidence, including when nested inside function types. Preserve reflection’s compile-time error if that evidence is absent. Confirmed during implementation.
- Real NaNs are unordered against everything, including NaNs. Signed zeros compare equal. Update `real::compare` and its six helpers; retain the old total ordering as `real::total_compare`.
- Compare arrays lexicographically, shorter equal prefixes first. Compare records by alphabetical field name and sum cases by alphabetical case name, followed by payloads when cases match. Alphabetical means case-sensitive Unicode scalar order without normalization. Stop at the first non-equal result, including unordered; later incomparable components do not invalidate an earlier ordering.
- Comparisons bind below arithmetic and above Boolean operators. Reject unparenthesized comparison chains.

Validation seams: source syntax/diagnostics and public library operations, exercised through compiled programs on the interpreter and JavaScript backend; formatter and editor grammar preserve the new syntax.

Implementation boundary discoveries:

- Unconstrained body-only type/row variables need concrete instantiations for reflection. Default only demanded variables absent from public and locally generalized contracts, using empty types/rows. Local allocation regions that cannot escape their enclosing function can likewise be instantiated. Preserve caller-owned and opened existential evidence requirements.
- Cell comparisons need opaque reflection descriptors retaining region and element identity. Add a descriptive Cell node; the typed shape exposes no cell operations, so comparison remains pure and unordered.
