Captured from the rebuilt Ruddy CLI on 2026-09-21, using the same isolated
copy of `../img/src/main.rud` as `current-signatures.md` (`@private` removed,
`std = false`). Every generated value annotation below was pasted onto its
original definition, and the resulting package passed `ruddy check`.

This iteration adds `when <= 'p` to annotations and simplifies existing Boolean
constraints. It introduces no new definition kind. Each inline marker denotes
an independent fresh presence implying the named bound.

| Function | Previous constraint lines | Current constraint lines | Whole declaration shorter by |
| --- | ---: | ---: | ---: |
| `extend4` | 25 | 5 | 61% |
| `get_channel` | 4 | 0 | 35% |
| `set_channel` | 10 | 8 | 35% |
| `swizzle` | 6 | 2 | 25% |
| `try_get_set` | 4 | 0 | 30% |

Character counts include any generated helper definitions in the same block.

The setter groups selectors sharing a field equality. This repeats fewer
expressions than grouping every consequence under each of four selectors.
`swizzle` uses 34 inline bounds: 32 selector cases and the final slot of each
input tuple. Shared presences and the two remaining tuple-prefix chains retain
explicit names.

# img_design_probe

## Types

### Buf

```ruddy
type Buf = [Nat8]
```

### Channel

```ruddy
type Channel =
  | #Red
  | #Blue
  | #Green
  | #Alpha
```

### RGB

```ruddy
type RGB = { r: Buf, g: Buf, b: Buf }
```

### RGBA

```ruddy
type RGBA = { ..RGB, a: Buf }
```

## Values

### extend4

```ruddy
let extend4: ((#None (when 'input_0_0_none) | ..'a) when 'input_0_0, (#None (when 'input_0_1_none) | ..'b) when 'input_0_1, (#None (when 'input_0_2_none) | ..'c) when 'input_0_2, (#None (when 'input_0_3_none) | ..'d) when 'input_0_3) -> (#None (when 'output_0_none) | ..'a, #None (when 'output_1_none) | ..'b, #None (when 'output_2_none) | ..'c, #None (when 'output_3_none) | ..'d)
where
  'output_0_none = ('input_0_0 -> 'input_0_0_none);
  'output_1_none = ('input_0_1 -> 'input_0_1_none);
  'output_2_none = ('input_0_2 -> 'input_0_2_none);
  'output_3_none = ('input_0_3 -> 'input_0_3_none);
  'input_0_3 <= 'input_0_2 <= 'input_0_1 <= 'input_0_0
```

### get_channel

```ruddy
let get_channel: #Red (when <= 'input_1_r) | #Green (when <= 'input_1_g) | #Blue (when <= 'input_1_b) | #Alpha (when <= 'input_1_a) -> { r when 'input_1_r: 'a, g when 'input_1_g: 'a, b when 'input_1_b: 'a, a when 'input_1_a: 'a, ..'b } -> 'a
```

### set_channel

```ruddy
let set_channel: #Red (when 'input_0_red) | #Green (when 'input_0_green) | #Blue (when 'input_0_blue) | #Alpha (when 'input_0_alpha) -> 'a -> { r when 'input_2_r: 'a, g when 'input_2_g: 'a, b when 'input_2_b: 'a, a when 'input_2_a: 'a, ..'b } -> { r when 'output_r: 'a, g when 'output_g: 'a, b when 'output_b: 'a, a when 'output_a: 'a, ..'b }
where
  'input_0_red or 'input_0_blue or 'input_0_alpha -> 'input_2_g = 'output_g;
  'input_0_red or 'input_0_green or 'input_0_alpha -> 'input_2_b = 'output_b;
  'input_0_red or 'input_0_green or 'input_0_blue -> 'output_a = 'input_2_a;
  'input_0_green or 'input_0_blue or 'input_0_alpha -> 'input_2_r = 'output_r;
  'input_0_red -> 'output_r;
  'input_0_green -> 'output_g;
  'input_0_blue -> 'output_b;
  'input_0_alpha -> 'output_a
```

### swizzle

```ruddy
let swizzle: ((#None? | #Red (when <= 'input_2_r) | #Green (when <= 'input_2_g) | #Blue (when <= 'input_2_b) | #Alpha (when <= 'input_2_a)) when 'input_0_0, (#None? | #Red (when <= 'input_2_r) | #Green (when <= 'input_2_g) | #Blue (when <= 'input_2_b) | #Alpha (when <= 'input_2_a)) when 'input_0_1, (#None? | #Red (when <= 'input_2_r) | #Green (when <= 'input_2_g) | #Blue (when <= 'input_2_b) | #Alpha (when <= 'input_2_a)) when 'input_0_2, (#None? | #Red (when <= 'input_2_r) | #Green (when <= 'input_2_g) | #Blue (when <= 'input_2_b) | #Alpha (when <= 'input_2_a)) when <= 'input_0_2) -> ((#None? | #Red (when <= 'input_2_r) | #Green (when <= 'input_2_g) | #Blue (when <= 'input_2_b) | #Alpha (when <= 'input_2_a)) when 'input_1_0, (#None? | #Red (when <= 'input_2_r) | #Green (when <= 'input_2_g) | #Blue (when <= 'input_2_b) | #Alpha (when <= 'input_2_a)) when 'input_1_1, (#None? | #Red (when <= 'input_2_r) | #Green (when <= 'input_2_g) | #Blue (when <= 'input_2_b) | #Alpha (when <= 'input_2_a)) when 'input_1_2, (#None? | #Red (when <= 'input_2_r) | #Green (when <= 'input_2_g) | #Blue (when <= 'input_2_b) | #Alpha (when <= 'input_2_a)) when <= 'input_1_2) -> { r when 'input_2_r: 'a, g when 'input_2_g: 'a, b when 'input_2_b: 'a, a when 'input_2_a: 'a, ..'b } -> { r when 'input_2_r: 'a, g when 'input_2_g: 'a, b when 'input_2_b: 'a, a when 'input_2_a: 'a, ..'b }
where
  'input_0_2 <= 'input_0_1 <= 'input_0_0;
  'input_1_2 <= 'input_1_1 <= 'input_1_0
```

### test

```ruddy
let test: { r: ['a], g: ['a], b: ['a] }
```

### try_get_set

```ruddy
let try_get_set: #None | #Red (when <= 'input_2_r) | #Green (when <= 'input_2_g) | #Blue (when <= 'input_2_b) | #Alpha (when <= 'input_2_a) -> #None | #Red (when <= 'input_2_r) | #Green (when <= 'input_2_g) | #Blue (when <= 'input_2_b) | #Alpha (when <= 'input_2_a) -> { r when 'input_2_r: 'a, g when 'input_2_g: 'a, b when 'input_2_b: 'a, a when 'input_2_a: 'a, ..'b } -> { r when 'input_2_r: 'a, g when 'input_2_g: 'a, b when 'input_2_b: 'a, a when 'input_2_a: 'a, ..'b }
```

<!-- Generated by ruddy doc for img_design_probe. -->
