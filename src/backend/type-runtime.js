// Only authentic Any packages own payloads. Their visible shape carries no
// writable descriptor that foreign code could forge or replace.
const $anyPackages = new WeakMap();
const $instantiateType = ($template, $arguments) => {
  if (!$arguments.length && !$template.nodes.some($node => typeof $node === "object" && "Extend" in $node)) return $template;
  let $root = $template.nodes[0];
  while (typeof $root === "object" && "Alias" in $root) $root = $template.nodes[$root.Alias];
  if (typeof $root === "object" && "Parameter" in $root) return $arguments[$root.Parameter];
  const $nodes = $template.nodes.slice();
  const $offsets = $arguments.map($argument => {
    const $offset = $nodes.length;
    for (const $node of $argument.nodes) {
      if (typeof $node === "string" || "Fixed" in $node) $nodes.push($node);
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
    let $kind;
    while ($work.length) {
      const $at = $typeIndex({ nodes: $nodes }, $work.pop());
      if ($seen.has($at)) throw new TypeError("cyclic runtime row extension");
      $seen.add($at);
      const $node = $nodes[$at];
      if ($node.Extend) { $work.push(...$node.Extend); continue; }
      const $shape = Object.keys($node)[0];
      if (($shape !== "Struct" && $shape !== "Sum") || ($kind && $kind !== $shape)) throw new TypeError("invalid runtime row extension");
      $kind = $shape;
      $fields.push(...$node[$shape]);
    }
    $fields.sort(($a, $b) => $a[0] < $b[0] ? -1 : $a[0] > $b[0] ? 1 : 0);
    if ($fields.some(($field, $at) => $at && $fields[$at - 1][0] === $field[0])) throw new TypeError("duplicate runtime row field");
    $nodes[$i] = { [$kind]: $fields };
  }
  return { nodes: $nodes };
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
const $convertType = ($descriptor, $value, $outgoing, $rootIndex = 0, $callable = true) => {
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
    const $node = $descriptor.nodes[$typeIndex($descriptor, $task.index)];
    if (typeof $node === "string") {
      let $valid = true;
      switch ($node) {
        case "Nat": $valid = Number.isSafeInteger($input) && $input >= 0; break;
        case "Int": $valid = Number.isSafeInteger($input); break;
        case "Real": $valid = typeof $input === "number"; break;
        case "String": $valid = typeof $input === "string"; break;
        case "Boolean": $valid = typeof $input === "boolean"; break;
        case "Any": $valid = $anyPackages.has($input); break;
        case "JsValue": break;
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
      if ($outgoing) {
        $put($argument => {
          const $result = $input($convertType($descriptor, $argument, false, $from));
          const $pending = $promiseCallbacks.has($input) || ($input[$closureMark] && !$input.nativeType && $f[$input.f].suspends);
          return $pending
            ? $result.then($value => $convertType($descriptor, $value, true, $to))
            : $convertType($descriptor, $result, true, $to);
        });
      } else {
        const $value = $argument => $invoke($value, "Sync", [$argument]);
        $value[$closureMark] = true;
        $value.nativeType = { descriptor: $descriptor, from: $from, to: $to, function: $input };
        $put($value);
      }
      continue;
    }
    if ("Struct" in $node && !$node.Struct.length && $input === undefined) {
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
        if (!Object.hasOwn($input, $name)) $fail($path + "." + $name, "a present field");
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

const $jsDecode = ($descriptor, $value) => {
  try { return $sum("Some", $convertType($descriptor, $value, false, 0, false)); }
  catch ($error) {
    const $failure = $conversionErrors.get($error) || { path: "$", expected: "readable native data", message: "JavaScript observation failed" };
    return $sum("Error", $record([
      ["path", $failure.path], ["expected", $failure.expected], ["message", $failure.message],
    ]));
  }
};
