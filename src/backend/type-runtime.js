// Adapter identity is keyed by the source value and the reviewed conversion
// position. Recursive host interfaces reuse their wrapper on a back edge.
const $convertedFunctions = new WeakMap();
// A value of a hidden type crosses a foreign boundary as a sealed package:
// the host is handed a frozen handle for it, and only such a handle is taken
// back. Nothing of the body is visible to or writable by the host, and no
// package can be made outside the program.
const $packages = new WeakMap();
const $handles = new WeakMap();
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
      if (typeof $node === "string" || "Fixed" in $node || "Parameter" in $node || "HiddenBound" in $node) $nodes.push($node);
      else if ("Array" in $node) $nodes.push({ Array: $node.Array + $offset });
      else if ("Mirror" in $node) $nodes.push({ Mirror: $node.Mirror + $offset });
      else if ("Hidden" in $node) $nodes.push({ Hidden: $node.Hidden + $offset });
      else if ("Alias" in $node) $nodes.push({ Alias: $node.Alias + $offset });
      else if ("Arrow" in $node) $nodes.push({ Arrow: $node.Arrow.map($index => $index + $offset) });
      else if ("Effects" in $node) $nodes.push({ Effects: $node.Effects.map($effect => ({ identity: $effect.identity, payload: $effect.payload + $offset, args: $effect.args.map($arg => $arg + $offset) })) });
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
    if (typeof $node === "string" || "Fixed" in $node || "Parameter" in $node || "HiddenBound" in $node) return $node;
    if ("Alias" in $node) return { Alias: $node.Alias + 1 };
    if ("Array" in $node) return { Array: $node.Array + 1 };
    if ("Mirror" in $node) return { Mirror: $node.Mirror + 1 };
    if ("Hidden" in $node) return { Hidden: $node.Hidden + 1 };
    if ("Arrow" in $node) return { Arrow: $node.Arrow.map($child => $child + 1) };
    if ("Effects" in $node) return { Effects: $node.Effects.map($effect => ({ identity: $effect.identity, payload: $effect.payload + 1, args: $effect.args.map($arg => $arg + 1) })) };
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
    if (typeof $node === "string" || "Fixed" in $node || "HiddenBound" in $node) continue;
    if ("Parameter" in $node) { $slots.add($node.Parameter); continue; }
    if ($defer && "Arrow" in $node) continue;
    const $children = "Alias" in $node ? [$node.Alias] : "Array" in $node ? [$node.Array] : "Mirror" in $node ? [$node.Mirror] : "Hidden" in $node ? [$node.Hidden] : "Arrow" in $node ? $node.Arrow : "Extend" in $node ? $node.Extend : "Effects" in $node ? $node.Effects.flatMap($effect => [$effect.payload, ...$effect.args]) : Object.values($node)[0].map($field => $field[1]);
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
    if ($kind === "Fixed" || $kind === "Parameter" || $kind === "HiddenBound") {
      if ($x[$kind] !== $y[$kind]) return false;
    } else if ($kind === "Array" || $kind === "Mirror" || $kind === "Hidden") {
      $work.push([$x[$kind], $y[$kind]]);
    } else if ($kind === "Arrow") {
      $work.push([$x.Arrow[0], $y.Arrow[0]], [$x.Arrow[1], $y.Arrow[1]], [$x.Arrow[2], $y.Arrow[2]]);
    } else if ($kind === "Effects") {
      if ($x.Effects.length !== $y.Effects.length) return false;
      for (let $i = 0; $i < $x.Effects.length; $i++) {
        const $a = $x.Effects[$i], $b = $y.Effects[$i];
        if ($a.identity !== $b.identity || $a.args.length !== $b.args.length) return false;
        $work.push([$a.payload, $b.payload]);
        for (let $j = 0; $j < $a.args.length; $j++) $work.push([$a.args[$j], $b.args[$j]]);
      }
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
// Mirrors are the descriptors the compiler already passes as evidence, made
// authentic: only a descriptor that came through one of these operations is
// a mirror, so foreign data cannot forge one.
const $mirrors = new WeakSet();
const $mirror = ($descriptor, $value) => {
  if (!$descriptor) throw new TypeError("missing runtime type information for a mirror");
  $mirrors.add($descriptor);
  return $descriptor;
};
const $typeOf = $mirror;
// A Ruddy function implemented here: it is handed the value as it is and
// hands back what it makes, with nothing converted in either direction.
const $nativeType = { nodes: [{ Arrow: [1, 1] }, "ForeignValue"] };
const $native = $function => {
  const $value = $argument => $invoke($value, "Sync", [$argument]);
  $value[$closureMark] = true;
  $value.nativeType = { descriptor: $nativeType, from: 1, to: 1, function: $function, slots: [] };
  return $value;
};
// A function of one value handed back unchanged: what an established equality
// of two mirrors supplies in each direction. It converts nothing, so it keeps
// the value's identity as well as its type.
const $passThrough = () => $native($x => $x);
// A mirror of one node of a mirror's graph: the field, element, or payload
// type, authentic because the whole was.
const $mirrorAt = ($of, $index) => {
  const $part = $typeAt($of, $index);
  $mirrors.add($part);
  return $part;
};
const $sameMirror = ($descriptor, $pair) => $sameType($pair["0"], $pair["1"])
  ? $sum("Some", $record([["forward", $passThrough()], ["backward", $passThrough()]]))
  : $sum("None", undefined);
// A mirror's graph as ordinary data: one node per descriptor node, indices
// kept, so recursion stays finite and nothing here can be turned back into
// a mirror.
const $describe = ($descriptor, $of) => {
  const $node = $n => {
    if ($n === "Nat" || $n === "Int") {
      const $bounds = $n === "Nat" ? $domains.nat : $domains.int;
      return $sum($n, $record([["bits", $bounds.bits], ["signed", $bounds.signed], ["min", $bounds.min], ["max", $bounds.max]]));
    }
    if (typeof $n === "string") return $sum($n === "ForeignValue" ? "Foreign" : $n, undefined);
    if ("Fixed" in $n) {
      const $bits = Number($n.Fixed.match(/\d+/)[0]), $signed = $n.Fixed.startsWith("Int");
      const $size = 1n << BigInt($bits - ($signed ? 1 : 0));
      return $sum("Fixed", $record([["bits", $bits], ["signed", $signed], ["min", ($signed ? -$size : 0n).toString()], ["max", ($size - 1n).toString()]]));
    }
    if ("Array" in $n) return $sum("Array", $n.Array);
    if ("Arrow" in $n) {
      const $effects = $of.nodes[$typeIndex($of, $n.Arrow[2])].Effects || [];
      return $sum("Function", $record([["argument", $n.Arrow[0]], ["result", $n.Arrow[1]], ["effects", $array($effects.map($effect => $record([["name", $effect.identity], ["node", $effect.payload]])))]]));
    }
    if ("Effects" in $n) return $sum("Effects", $array($n.Effects.map($effect => $record([["name", $effect.identity], ["node", $effect.payload]]))));
    if ("Struct" in $n || "Sum" in $n) {
      const $fields = "Struct" in $n ? $n.Struct : $n.Sum;
      return $sum("Struct" in $n ? "Record" : "Sum", $array($fields.map(([$name, $index]) => $record([["name", $name], ["node", $index]]))));
    }
    if ("Alias" in $n) return $sum("Alias", $n.Alias);
    if ("Extend" in $n) return $sum("Extend", $record([["base", $n.Extend[0]], ["rest", $n.Extend[1]]]));
    if ("Parameter" in $n) return $sum("Parameter", $n.Parameter);
    if ("Mirror" in $n) return $sum("Mirror", $n.Mirror);
    if ("Hidden" in $n) return $sum("Hidden", $n.Hidden);
    return $sum("Variable", $n.HiddenBound);
  };
  return $record([["root", 0], ["nodes", $array($of.nodes.map($node))]]);
};
// A mirror's outermost structure as `std::reflect::Shape`: typed views whose
// operations read and make values of the mirrored type. The mirror proves
// the type, so the operations convert nothing; they only take values apart
// and put them together.
const $shape = ($descriptor, $of) => {
  const $node = $of.nodes[$typeIndex($of, 0)];
  const $typed = $name => $sum($name, $record([["read", $passThrough()], ["make", $passThrough()]]));
  if (typeof $node === "string") {
    return $node === "ForeignValue" ? $sum("Foreign", undefined) : $typed($node);
  }
  if ("Fixed" in $node) return $typed($node.Fixed);
  if ("Array" in $node) {
    return $sum("Array", $record([["element", $mirrorAt($of, $node.Array)], ["read", $passThrough()], ["make", $passThrough()]]));
  }
  if ("Struct" in $node) {
    const $fields = $node.Struct.map(([$name, $index]) => {
      const $field = $mirrorAt($of, $index);
      return $record([
        ["name", $name], ["mirror", $field], ["presence", $sum("Required", undefined)],
        ["read", $native($value => $sum("Some", $value[$name]))],
        ["bind", $native($value => $record([["record", $of], ["name", $name], ["mirror", $field], ["value", $value]]))],
      ]);
    });
    // A binding is accepted on what it proves, not where it came from: an
    // equivalent mirror of this record and of the field it names.
    const $build = $native($bindings => {
      const $bound = new Map();
      for (const $binding of $arrayValues($bindings)) {
        const $name = $binding.name, $field = $node.Struct.find(([$known]) => $known === $name);
        const $reject = $reason => $sum("Error", $sum($reason, $name));
        if (!$sameType($binding.record, $of)) return $reject("Foreign");
        if (!$field) return $reject("Unknown");
        if (!$sameType($binding.mirror, $typeAt($of, $field[1]))) return $reject("Mismatched");
        if ($bound.has($name)) return $reject("Duplicate");
        $bound.set($name, $binding.value);
      }
      const $missing = $node.Struct.find(([$name]) => !$bound.has($name));
      if ($missing) return $sum("Error", $sum("Missing", $missing[0]));
      return $sum("Some", $record($node.Struct.map(([$name]) => [$name, $bound.get($name)])));
    });
    return $sum("Record", $record([["mirror", $of], ["fields", $array($fields)], ["build", $build]]));
  }
  if ("Sum" in $node) {
    const $cases = $node.Sum.map(([$name, $index]) => $record([
      ["name", $name], ["mirror", $mirrorAt($of, $index)],
      ["project", $native($value => $value[$tag] === $name
        ? $sum("Some", $value[$payload] === undefined ? $record([]) : $value[$payload])
        : $sum("None", undefined))],
      ["inject", $sum("Some", $native($value => $sum($name, $value)))],
    ]));
    return $sum("Sum", $record([["mirror", $of], ["cases", $array($cases)]]));
  }
  if ("Arrow" in $node) return $sum("Function", $describe($of, $of));
  if ("Hidden" in $node) return $sum("Hidden", $describe($of, $of));
  if ("Mirror" in $node) return $sum("Mirror", $describe($of, $of));
  throw new TypeError("unknown runtime type node");
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
      // A target integer is checked against the bound domain, and integers
      // have one zero: a negative zero from outside becomes zero.
      let $value = $input;
      switch ($node) {
        case "Nat": $valid = Number.isInteger($input) && $input >= 0 && $input <= $domains.nat.high; if ($valid) $value = $input + 0; break;
        case "Int": $valid = Number.isInteger($input) && $input >= $domains.int.low && $input <= $domains.int.high; if ($valid) $value = $input + 0; break;
        case "Real": $valid = typeof $input === "number"; break;
        case "String": $valid = typeof $input === "string"; break;
        case "Bool": $valid = typeof $input === "boolean"; break;
        case "ForeignValue": break;
        default: $valid = false;
      }
      if (!$valid) $fail($path, $node);
      $put($value);
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
    if ("Mirror" in $node) {
      if (!$mirrors.has($input)) $fail($path, "an authentic mirror");
      $put($input);
      continue;
    }
    if ("Hidden" in $node) {
      if ($outgoing) {
        const $sealable = typeof $input === "function" || (typeof $input === "object" && $input !== null);
        let $handle = $sealable ? $handles.get($input) : undefined;
        if (!$handle) {
          $handle = Object.freeze(Object.create(null));
          $packages.set($handle, $input);
          if ($sealable) $handles.set($input, $handle);
        }
        $put($handle);
      } else {
        if (!$packages.has($input)) $fail($path, "a package this program made");
        $put($packages.get($input));
      }
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
        // Runtime intrinsics can retain a foreign callback wrapper in a
        // container. Host exports call its source closure so hidden effect
        // evidence is supplied, while explicit source callback modes survive.
        const $source = $hostExport && $foreignCallbacks.has($input) ? $fromHost($input) : $input;
        $remember($argument => {
          const $converted = $convertType($descriptor, $argument, false, $from, $callable, $hostExport);
          // A descriptor supplies a pure synchronous callable contract. A
          // conservative code-level suspension summary must not turn an
          // immediately completed callback's data into a Promise.
          const $result = $source[$closureMark] && !$hostExport
            ? $invoke($source, "Sync", [$converted])
            : $source($converted);
          const $pending = $promiseCallbacks.has($source) || ($hostExport && $source[$closureMark] && !$source.nativeType && $f[$source.f].suspends);
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
