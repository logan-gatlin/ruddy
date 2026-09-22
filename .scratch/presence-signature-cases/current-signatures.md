Captured from the existing Ruddy CLI on 2026-09-21, from an isolated copy of `../img/src/main.rud` with `@private` removed and `std = false`. The source functions are unchanged.

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
  'input_0_3 <= 'input_0_2 <= 'input_0_1 <= 'input_0_0;
  'input_0_0 or 'output_0_none;
  'input_0_0 or 'output_1_none;
  'input_0_0 or 'output_2_none;
  'input_0_0 or 'output_3_none;
  'input_0_0 and 'output_0_none or 'input_0_1 and 'output_0_none or 'input_0_2 and 'output_0_none or 'input_0_3 and 'output_0_none -> 'input_0_0_none;
  'input_0_1 or 'output_1_none;
  'input_0_1 or 'output_2_none;
  'input_0_1 or 'output_3_none;
  'input_0_1 and 'output_1_none or 'input_0_2 and 'output_1_none or 'input_0_3 and 'output_1_none -> 'input_0_1_none;
  'input_0_2 or 'output_2_none;
  'input_0_2 or 'output_3_none;
  'input_0_2 and 'output_2_none or 'input_0_3 and 'output_2_none -> 'input_0_2_none;
  'input_0_3 or 'output_3_none;
  'input_0_3 and 'output_3_none -> 'input_0_3_none;
  'input_0_0_none -> 'output_0_none;
  ('output_0_none -> 'output_1_none) or 'input_0_0_none;
  ('output_0_none -> 'output_2_none) or 'input_0_0_none;
  ('output_0_none -> 'output_3_none) or 'input_0_0_none;
  'input_0_1_none -> 'output_1_none;
  ('output_1_none -> 'output_2_none) or 'input_0_1_none;
  ('output_1_none -> 'output_3_none) or 'input_0_1_none;
  'input_0_2_none -> 'output_2_none;
  ('output_2_none -> 'output_3_none) or 'input_0_2_none;
  'input_0_3_none -> 'output_3_none
```

### get_channel

```ruddy
let get_channel: #Red (when 'input_0_red) | #Green (when 'input_0_green) | #Blue (when 'input_0_blue) | #Alpha (when 'input_0_alpha) -> { r when 'input_1_r: 'a, g when 'input_1_g: 'a, b when 'input_1_b: 'a, a when 'input_1_a: 'a, ..'b } -> 'a
where
  'input_0_red -> 'input_1_r;
  'input_0_green -> 'input_1_g;
  'input_0_blue -> 'input_1_b;
  'input_0_alpha -> 'input_1_a
```

### set_channel

```ruddy
let set_channel: #Red (when 'input_0_red) | #Green (when 'input_0_green) | #Blue (when 'input_0_blue) | #Alpha (when 'input_0_alpha) -> 'a -> { r when 'input_2_r: 'a, g when 'input_2_g: 'a, b when 'input_2_b: 'a, a when 'input_2_a: 'a, ..'b } -> { r when 'output_r: 'a, g when 'output_g: 'a, b when 'output_b: 'a, a when 'output_a: 'a, ..'b }
where
  'input_0_red -> ('input_2_g = 'output_g) and ('input_2_b = 'output_b) and ('output_a = 'input_2_a) and 'output_r;
  'input_0_green -> ('input_2_r = 'output_r) and ('input_2_b = 'output_b) and ('output_a = 'input_2_a) and 'output_g;
  'input_0_blue -> ('input_2_r = 'output_r) and ('input_2_g = 'output_g) and ('output_a = 'input_2_a) and 'output_b;
  'input_0_alpha -> ('input_2_r = 'output_r) and ('input_2_g = 'output_g) and ('input_2_b = 'output_b) and 'output_a;
  'input_0_red and 'input_0_green -> 'input_2_r and 'input_2_g;
  'input_0_red and 'input_0_blue -> 'input_2_r and 'input_2_b;
  'input_0_red and 'input_0_alpha -> 'input_2_r and 'input_2_a;
  'input_0_green and 'input_0_blue -> 'input_2_g and 'input_2_b;
  'input_0_green and 'input_0_alpha -> 'input_2_g and 'input_2_a;
  'input_0_blue and 'input_0_alpha -> 'input_2_b and 'input_2_a
```

### swizzle

```ruddy
type InferredChoice 'a 'b 'c 'd 'e = #None (when 'a) | #Red (when 'b) | #Green (when 'c) | #Blue (when 'd) | #Alpha (when 'e)
let swizzle: ((InferredChoice _ 'input_0_0_red 'input_0_0_green 'input_0_0_blue 'input_0_0_alpha) when 'input_0_0, (InferredChoice _ 'input_0_1_red 'input_0_1_green 'input_0_1_blue 'input_0_1_alpha) when 'input_0_1, (InferredChoice _ 'input_0_2_red 'input_0_2_green 'input_0_2_blue 'input_0_2_alpha) when 'input_0_2, (InferredChoice _ 'input_0_3_red 'input_0_3_green 'input_0_3_blue 'input_0_3_alpha) when 'input_0_3) -> ((InferredChoice _ 'input_1_0_red 'input_1_0_green 'input_1_0_blue 'input_1_0_alpha) when 'input_1_0, (InferredChoice _ 'input_1_1_red 'input_1_1_green 'input_1_1_blue 'input_1_1_alpha) when 'input_1_1, (InferredChoice _ 'input_1_2_red 'input_1_2_green 'input_1_2_blue 'input_1_2_alpha) when 'input_1_2, (InferredChoice _ 'input_1_3_red 'input_1_3_green 'input_1_3_blue 'input_1_3_alpha) when 'input_1_3) -> { r when 'input_2_r: 'a, g when 'input_2_g: 'a, b when 'input_2_b: 'a, a when 'input_2_a: 'a, ..'b } -> { r when 'input_2_r: 'a, g when 'input_2_g: 'a, b when 'input_2_b: 'a, a when 'input_2_a: 'a, ..'b }
where
  'input_0_3 <= 'input_0_2 <= 'input_0_1 <= 'input_0_0;
  'input_0_0_red or 'input_0_1_red or 'input_0_2_red or 'input_0_3_red or 'input_1_0_red or 'input_1_1_red or 'input_1_2_red or 'input_1_3_red -> 'input_2_r;
  'input_0_0_green or 'input_0_1_green or 'input_0_2_green or 'input_0_3_green or 'input_1_0_green or 'input_1_1_green or 'input_1_2_green or 'input_1_3_green -> 'input_2_g;
  'input_0_0_blue or 'input_0_1_blue or 'input_0_2_blue or 'input_0_3_blue or 'input_1_0_blue or 'input_1_1_blue or 'input_1_2_blue or 'input_1_3_blue -> 'input_2_b;
  'input_0_0_alpha or 'input_0_1_alpha or 'input_0_2_alpha or 'input_0_3_alpha or 'input_1_0_alpha or 'input_1_1_alpha or 'input_1_2_alpha or 'input_1_3_alpha -> 'input_2_a;
  'input_1_3 <= 'input_1_2 <= 'input_1_1 <= 'input_1_0
```

### test

```ruddy
let test: { r: ['a], g: ['a], b: ['a] }
```

### try_get_set

```ruddy
let try_get_set: #None | #Red (when 'input_0_red) | #Green (when 'input_0_green) | #Blue (when 'input_0_blue) | #Alpha (when 'input_0_alpha) -> #None | #Red (when 'input_1_red) | #Green (when 'input_1_green) | #Blue (when 'input_1_blue) | #Alpha (when 'input_1_alpha) -> { r when 'input_2_r: 'a, g when 'input_2_g: 'a, b when 'input_2_b: 'a, a when 'input_2_a: 'a, ..'b } -> { r when 'input_2_r: 'a, g when 'input_2_g: 'a, b when 'input_2_b: 'a, a when 'input_2_a: 'a, ..'b }
where
  'input_0_red or 'input_1_red -> 'input_2_r;
  'input_0_green or 'input_1_green -> 'input_2_g;
  'input_0_blue or 'input_1_blue -> 'input_2_b;
  'input_0_alpha or 'input_1_alpha -> 'input_2_a
```

<!-- Generated by ruddy doc for img_design_probe. -->
