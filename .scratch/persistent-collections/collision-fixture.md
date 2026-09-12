# Full-hash collision fixture

`tests/fixtures/hamt-collision.json` contains two unequal printable ASCII strings
with the same current `std::hash` result. The public collection test verifies
that premise in both runtimes before exercising collision buckets. The fixture
does not establish a hash stability contract; update it if the hash changes.

The strings were constructed algebraically for FNV-1a64, with prime
`P = 1099511628211`. Both use the normal string framing: byte `3`, the length
as eight little-endian bytes, then UTF-8 bytes. The length is 40 and the prefix
`" n"` brings the running state's low byte to zero.

For a state with low byte zero, the two-byte block `[32 + 10t, 96 - 2t]`
(`0 <= t <= 9`) preserves that property. Relative to the block for `t = 0`,
its contribution is `256 * P * (10 * 2^32 + 17) * t` modulo `2^64`.
Successive blocks multiply previous contributions by `P^2`.

Integer lattice reduction found the following coefficients whose weighted
sum against `(P^2)^i` is zero modulo `2^56`:

```
[1, 1, 2, 1, 4, 1, 4, 5, 2, -2, -2, -2, 0, 0, -4, 2, -1, -1, -3]
```

Traverse these coefficients in reverse order. Use each positive part as `t`
for the left string and each negative part's magnitude for the right string.
The resulting framed hashes both equal `14863065250766243072`.
No lattice library or collision-generation tool is a project dependency.
