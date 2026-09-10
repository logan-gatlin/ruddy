// Adapter identity is keyed by the source value and the reviewed conversion
// position. Recursive host interfaces reuse their wrapper on a back edge.
const $convertedFunctions = new WeakMap();
// Only authentic Any packages own payloads. Their visible shape carries no
// writable descriptor that foreign code could forge or replace.
const $anyPackages = new WeakMap();
const $instantiateType = ($template, $arguments, $partial = false) => {
  if ($arguments.some($argument => $argument === undefined)) return undefined;
  if (!$arguments.length && !$template.nodes.some($node => typeof $node === "object" && "Extend" in $node)) return $template;
  let $root = $template.nodes[0];
  while (typeof $root === "object" && "Alias" in $root) $root = $template.nodes[$root.Alias];
  if (typeof $root === "object" && "Parameter" in $root) return $arguments[$root.Parameter];
  const $nodes = $template.nodes.slice();
  const $offsets = $arguments.map($argument => {
    const $offset = $nodes.length;
    for (const $node of $argument.nodes) {
      if (typeof $node === "string" || "Fixed" in $node || "Parameter" in $node) $nodes.push($node);
      else if ("Array" in $node) $nodes.push({ Array: $node.Array + $offset });
      else if ("Alias" in $node) $nodes.push({ Alias: $node.Alias + $offset });
      else if ("Arrow" in $node) $nodes.push({ Arrow: $node.Arrow.map($index => $index + $offset) });
      else {
        const $kind = Object.keys($node)[0];
        $nodes.push({ [$kind]: $node[$kind].map(([$name, $index]) => [$name, $index + $offset]) });
      }
    }
    return $offset;
  });
  for (let $i = 0; $i < $template.nodes.length; $i++) {
    const $node = $nodes[$i];
    if (typeof $node === "object" && "Parameter" in $node) {
      $nodes[$i] = { Alias: $offsets[$node.Parameter] };
    }
  }
  for (let $i = 0; $i < $template.nodes.length; $i++) {
    if (!($nodes[$i] && typeof $nodes[$i] === "object" && "Extend" in $nodes[$i])) continue;
    const $fields = [], $work = [$i], $seen = new Set();
    let $kind, $deferred = false;
    while ($work.length) {
      const $at = $typeIndex({ nodes: $nodes }, $work.pop());
      if ($seen.has($at)) throw new TypeError("cyclic runtime row extension");
      $seen.add($at);
      const $node = $nodes[$at];
      if ($partial && typeof $node === "object" && "Parameter" in $node) { $deferred = true; break; }
      if ($node.Extend) { $work.push(...$node.Extend); continue; }
      const $shape = Object.keys($node)[0];
      if (($shape !== "Struct" && $shape !== "Sum") || ($kind && $kind !== $shape)) throw new TypeError("invalid runtime row extension");
      $kind = $shape;
      $fields.push(...$node[$shape]);
    }
    if ($deferred) continue;
    $fields.sort(($a, $b) => $a[0] < $b[0] ? -1 : $a[0] > $b[0] ? 1 : 0);
    if ($fields.some(($field, $at) => $at && $fields[$at - 1][0] === $field[0])) throw new TypeError("duplicate runtime row field");
    $nodes[$i] = { [$kind]: $fields };
  }
  return { nodes: $nodes };
};
// Conversion plans may retain holes underneath returned functions. They are
// separate from exact descriptors and can never establish an Any identity.
const $typeAt = ($descriptor, $index) => ({ nodes: [
  { Alias: $index + 1 },
  ...$descriptor.nodes.map($node => {
    if (typeof $node === "string" || "Fixed" in $node || "Parameter" in $node) return $node;
    if ("Alias" in $node) return { Alias: $node.Alias + 1 };
    if ("Array" in $node) return { Array: $node.Array + 1 };
    if ("Arrow" in $node) return { Arrow: $node.Arrow.map($child => $child + 1) };
    if ("Extend" in $node) return { Extend: $node.Extend.map($child => $child + 1) };
    const $kind = Object.keys($node)[0];
    return { [$kind]: $node[$kind].map(([$name, $child]) => [$name, $child + 1]) };
  }),
] });
const $projectType = ($descriptor, $path) => {
  if ($descriptor === undefined) return undefined;
  let $index = 0;
  for (const $step of $path) {
    $index = $typeIndex($descriptor, $index);
    const $node = $descriptor.nodes[$index];
    if ($step === "Element") $index = $node.Array;
    else if ($step === "Argument") $index = $node.Arrow?.[0];
    else if ($step === "Result") $index = $node.Arrow?.[1];
    else if ("Field" in $step) $index = ($node.Struct || $node.Sum)?.find($field => $field[0] === $step.Field)?.[1];
    else if ("Remainder" in $step) {
      const $kind = "Struct" in $node ? "Struct" : "Sum";
      if (!$node[$kind]) throw new TypeError("invalid runtime row projection");
      $descriptor = $typeAt($descriptor, $index);
      $descriptor.nodes[0] = { [$kind]: $node[$kind].filter($field => !$step.Remainder.includes($field[0])).map(([$name, $child]) => [$name, $child + 1]) };
      $index = 0;
    }
    if ($index === undefined) throw new TypeError("invalid runtime type projection");
  }
  return $typeAt($descriptor, $index);
};
const $nativePlan = ($template, $arguments) => {
  const $schema = $template.descriptor || $template;
  const $partial = $arguments.map(($argument, $index) => $argument === undefined ? { nodes: [{ Parameter: $index }] } : $argument);
  const $descriptor = $instantiateType($schema, $partial, true);
  const $optional = { ...$template.optional_fields };
  // Row extensions inherit the optionality of their known constituent fields.
  for (let $index = 0; $index < $schema.nodes.length; $index++) {
    const $node = $schema.nodes[$index];
    if (typeof $node !== "object" || !("Extend" in $node)) continue;
    const $fields = new Set(), $pending = [$index], $seen = new Set();
    while ($pending.length) {
      const $at = $pending.pop();
      if ($seen.has($at)) continue;
      $seen.add($at);
      for (const $name of $optional[$at] || []) $fields.add($name);
      const $part = $schema.nodes[$at];
      if (typeof $part === "object" && "Alias" in $part) $pending.push($part.Alias);
      else if (typeof $part === "object" && "Extend" in $part) $pending.push(...$part.Extend);
    }
    if ($fields.size) $optional[$index] = [...$fields];
  }
  return { nodes: $descriptor.nodes, template: $template, arguments: $arguments, optional: $optional };
};
const $nativeSlots = ($descriptor, $index) => {
  if (!$descriptor.template) return [];
  const $nodes = ($descriptor.template.descriptor || $descriptor.template).nodes, $arrow = $nodes[$index];
  if (!$arrow || typeof $arrow !== "object" || !("Arrow" in $arrow)) return [];
  const $work = [[$arrow.Arrow[0], false], [$arrow.Arrow[1], true]], $seen = new Set(), $slots = new Set();
  while ($work.length) {
    const [$index, $defer] = $work.pop(), $key = $index + ":" + $defer;
    if ($seen.has($key)) continue;
    $seen.add($key);
    const $node = $nodes[$index];
    if (typeof $node === "string" || "Fixed" in $node) continue;
    if ("Parameter" in $node) { $slots.add($node.Parameter); continue; }
    if ($defer && "Arrow" in $node) continue;
    const $children = "Alias" in $node ? [$node.Alias] : "Array" in $node ? [$node.Array] : "Arrow" in $node ? $node.Arrow : "Extend" in $node ? $node.Extend : Object.values($node)[0].map($field => $field[1]);
    for (const $child of $children) $work.push([$child, $defer]);
  }
  return [...$slots].sort(($a, $b) => $a - $b);
};
const $typeIndex = ($descriptor, $index) => {
  while (typeof $descriptor.nodes[$index] === "object" && "Alias" in $descriptor.nodes[$index]) {
    $index = $descriptor.nodes[$index].Alias;
  }
  return $index;
};
const $sameType = ($left, $right) => {
  const $work = [[0, 0]], $seen = new Set();
  while ($work.length) {
    let [$a, $b] = $work.pop();
    $a = $typeIndex($left, $a);
    $b = $typeIndex($right, $b);
    const $key = $a + ":" + $b;
    if ($seen.has($key)) continue;
    $seen.add($key);
    const $x = $left.nodes[$a], $y = $right.nodes[$b];
    if (typeof $x === "string" || typeof $y === "string") {
      if ($x !== $y) return false;
      continue;
    }
    const $kind = Object.keys($x)[0];
    if ($kind !== Object.keys($y)[0]) return false;
    if ($kind === "Fixed") {
      if ($x.Fixed !== $y.Fixed) return false;
    } else if ($kind === "Array") {
      $work.push([$x.Array, $y.Array]);
    } else if ($kind === "Arrow") {
      $work.push([$x.Arrow[0], $y.Arrow[0]], [$x.Arrow[1], $y.Arrow[1]]);
    } else {
      const $xs = $x[$kind], $ys = $y[$kind];
      if ($xs.length !== $ys.length) return false;
      for (let $i = 0; $i < $xs.length; $i++) {
        if ($xs[$i][0] !== $ys[$i][0]) return false;
        $work.push([$xs[$i][1], $ys[$i][1]]);
      }
    }
  }
  return true;
};
const $anyUpcast = ($descriptor, $value) => {
  if (!$descriptor) throw new TypeError("missing runtime type information for Any");
  const $package = Object.freeze(Object.create(null));
  $anyPackages.set($package, { descriptor: $descriptor, value: $value });
  return $package;
};
const $anyDowncast = ($descriptor, $value) => {
  const $package = $anyPackages.get($value);
  return $package && $sameType($descriptor, $package.descriptor)
    ? $sum("Some", $package.value)
    : $sum("None", $record([]));
};

// Conversion uses an explicit work stack, including leave tasks for cycle
// detection. A repeated sibling reference is copied; a cyclic path is rejected.
const $conversionErrors = new WeakMap();
const $convertType = ($descriptor, $value, $outgoing, $rootIndex = 0, $callable = true, $hostExport = false) => {
  const $root = { value: undefined }, $active = new WeakSet();
  const $work = [{ index: $rootIndex, value: $value, path: "$", put: $v => { $root.value = $v; } }];
  const $fail = ($path, $expected) => {
    const $error = new TypeError("Foreign value at " + $path + " must be " + $expected);
    $conversionErrors.set($error, { path: $path, expected: $expected, message: $error.message });
    throw $error;
  };
  while ($work.length) {
    const $task = $work.pop();
    if ($task.complete) { $task.complete(); continue; }
    if ($task.leave) { $active.delete($task.leave); continue; }
    const { value: $input, path: $path, put: $put } = $task;
    const $shapeIndex = $typeIndex($descriptor, $task.index);
    const $node = $descriptor.nodes[$shapeIndex];
    if (typeof $node === "string") {
      let $valid = true;
      switch ($node) {
        case "Nat": $valid = Number.isInteger($input) && $input >= 0; break;
        case "Int": $valid = Number.isInteger($input); break;
        case "Real": $valid = typeof $input === "number"; break;
        case "String": $valid = typeof $input === "string"; break;
        case "Bool": $valid = typeof $input === "boolean"; break;
        case "Any": $valid = $anyPackages.has($input); break;
        case "ForeignValue": break;
        default: $valid = false;
      }
      if (!$valid) $fail($path, $node);
      $put($input);
      continue;
    }
    if ("Fixed" in $node) {
      const $name = $node.Fixed, $bits = Number($name.match(/\d+/)[0]), $signed = $name.startsWith("Int");
      const $size = 1n << BigInt($bits - ($signed ? 1 : 0));
      const $min = $signed ? -$size : 0n, $max = $size - 1n;
      if ($bits === 64 ? typeof $input !== "bigint" : !Number.isSafeInteger($input)) $fail($path, $name);
      if (BigInt($input) < $min || BigInt($input) > $max) $fail($path, $name);
      $put($input);
      continue;
    }
    if ("Arrow" in $node) {
      if (!$callable) $fail($path, "a verifiable data type; function contracts cannot be decoded");
      if (typeof $input !== "function") $fail($path, "Function");
      const [$from, $to] = $node.Arrow;
      let $adapters = $convertedFunctions.get($input);
      // A live host function must not keep every short-lived conversion plan
      // alive. The descriptor and its adapter may be collected together.
      if (!$adapters) { $adapters = new WeakMap(); $convertedFunctions.set($input, $adapters); }
      let $positions = $adapters.get($descriptor);
      if (!$positions) { $positions = new Map(); $adapters.set($descriptor, $positions); }
      const $key = $shapeIndex + ":" + $outgoing + ":" + $hostExport;
      const $existing = $positions.get($key);
      if ($existing) { $put($existing); continue; }
      const $remember = $value => {
        $positions.set($key, $value);
        $put($value);
      };
      if ($outgoing) {
        $remember($argument => {
          const $converted = $convertType($descriptor, $argument, false, $from, $callable, $hostExport);
          // A descriptor supplies a pure synchronous callable contract. A
          // conservative code-level suspension summary must not turn an
          // immediately completed callback's data into a Promise.
          const $result = $input[$closureMark] && !$hostExport
            ? $invoke($input, "Sync", [$converted])
            : $input($converted);
          const $pending = $promiseCallbacks.has($input) || ($hostExport && $input[$closureMark] && !$input.nativeType && $f[$input.f].suspends);
          return $pending
            ? $result.then($value => $convertType($descriptor, $value, true, $to, $callable, $hostExport))
            : $convertType($descriptor, $result, true, $to, $callable, $hostExport);
        });
      } else {
        const $value = $argument => $invoke($value, "Sync", [$argument]);
        $value[$closureMark] = true;
        $value.nativeType = { descriptor: $descriptor, from: $from, to: $to, function: $input, slots: $nativeSlots($descriptor, $shapeIndex) };
        $remember($value);
      }
      continue;
    }
    if ("Struct" in $node && !$node.Struct.length && (($callable && !$outgoing) || $input === undefined || $input === null)) {
      $put($outgoing ? undefined : $record([]));
      continue;
    }
    if (!$input || typeof $input !== "object") $fail($path, Object.keys($node)[0]);
    if ($active.has($input)) $fail($path, "an acyclic value");
    $active.add($input);
    $work.push({ leave: $input });
    if ("Array" in $node) {
      if (!$outgoing && !Array.isArray($input)) $fail($path, "Array");
      const $items = $outgoing ? Array.from($arrayValues($input)) : $input;
      const $result = new Array($items.length);
      // A completion task runs after every element has been copied.
      $work.push({ complete: () => $put($outgoing ? $result : $array($result)) });
      for (let $i = $items.length - 1; $i >= 0; $i--) {
        $work.push({ index: $node.Array, value: $items[$i], path: $path + "[" + $i + "]", put: $v => { $result[$i] = $v; } });
      }
    } else if ("Struct" in $node) {
      const $result = Object.create(null);
      $put($result);
      for (let $i = $node.Struct.length - 1; $i >= 0; $i--) {
        const [$name, $index] = $node.Struct[$i];
        if (!Object.hasOwn($input, $name)) {
          if ($descriptor.optional && ($descriptor.optional[$shapeIndex] || []).includes($name)) continue;
          $fail($path + "." + $name, "a present field");
        }
        $work.push({ index: $index, value: $input[$name], path: $path + "." + $name, put: $v => { $result[$name] = $v; } });
      }
    } else if ("Sum" in $node) {
      const $name = $outgoing ? $input[$tag] : Object.hasOwn($input, "tag") ? $input.tag : $input[$tag];
      const $field = $node.Sum.find($field => $field[0] === $name);
      if (!$field) $fail($path, "one of " + $node.Sum.map($field => $field[0]).join(", "));
      const $value = $outgoing || !Object.hasOwn($input, "tag") ? $input[$payload] : $input.value;
      $work.push({ index: $field[1], value: $value, path: $path + "." + $name, put: $v => $put($outgoing ? { tag: $name, value: $v } : $sum($name, $v)) });
    } else {
      $fail($path, "a supported native type");
    }
  }
  return $root.value;
};

const $ffiDecode = ($descriptor, $value) => {
  try { return $sum("Some", $convertType($descriptor, $value, false, 0, false)); }
  catch ($error) {
    const $failure = $conversionErrors.get($error) || { path: "$", expected: "readable native data", message: "JavaScript observation failed" };
    return $sum("Error", $record([
      ["path", $failure.path], ["expected", $failure.expected], ["message", $failure.message],
    ]));
  }
};
