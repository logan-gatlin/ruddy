// The portable primitives the standard library declares, by module and
// name: one contract each, implemented here for JavaScript and again by
// every other backend. Generated from the standard library's own text.
const $prim = {
  binary: {
    real_to_bits: (
      value => { const view = new DataView(new ArrayBuffer(8)); view.setFloat64(0, value, true); return view.getBigUint64(0, true); }
    ),
    real_from_bits: (
      bits => { const view = new DataView(new ArrayBuffer(8)); view.setBigUint64(0, bits, true); return view.getFloat64(0, true); }
    ),
    utf8_encode: (
      text => Array.from(new TextEncoder().encode(text))
    ),
    utf8_decode: (
      bytes => { try { return { tag: "Some", value: new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(bytes)) }; } catch (error) { return { tag: "None" }; } }
    ),
    nat64_to_bytes: (
      value => Array.from({ length: 8 }, (_, index) => Number((value >> BigInt(index * 8)) & 255n))
    ),
    nat64_from_bytes: (
      bytes => bytes.reduce((total, byte, index) => total | (BigInt(byte) << BigInt(index * 8)), 0n)
    ),
    int64_to_bytes: (
      value => { const unsigned = BigInt.asUintN(64, value); return Array.from({ length: 8 }, (_, index) => Number((unsigned >> BigInt(index * 8)) & 255n)); }
    ),
    int64_from_bytes: (
      bytes => BigInt.asIntN(64, bytes.reduce((total, byte, index) => total | (BigInt(byte) << BigInt(index * 8)), 0n))
    ),
    nat_to_bytes: (
      value => Array.from({ length: 8 }, (_, index) => Number((BigInt(value) >> BigInt(index * 8)) & 255n))
    ),
    int_to_bytes: (
      value => { const unsigned = BigInt.asUintN(64, BigInt(value)); return Array.from({ length: 8 }, (_, index) => Number((unsigned >> BigInt(index * 8)) & 255n)); }
    ),
    nat_from_bytes: (
      bytes => { const value = bytes.reduce((total, byte, index) => total | (BigInt(byte) << BigInt(index * 8)), 0n); return value <= BigInt(Number.MAX_SAFE_INTEGER) ? { tag: "Some", value: Number(value) } : { tag: "None" }; }
    ),
    int_from_bytes: (
      bytes => { const value = BigInt.asIntN(64, bytes.reduce((total, byte, index) => total | (BigInt(byte) << BigInt(index * 8)), 0n)); return value <= BigInt(Number.MAX_SAFE_INTEGER) && value >= -BigInt(Number.MAX_SAFE_INTEGER) ? { tag: "Some", value: Number(value) } : { tag: "None" }; }
    ),
    nat32_to_bytes: (
      value => [value & 255, (value >>> 8) & 255, (value >>> 16) & 255, (value >>> 24) & 255]
    ),
    nat32_from_bytes: (
      bytes => (bytes[0] | (bytes[1] << 8) | (bytes[2] << 16)) + bytes[3] * 16777216
    ),
  },
  int: {
    add: (
      (left, right) => left + right
    ),
    subtract: (
      (left, right) => left - right
    ),
    multiply: (
      (left, right) => left * right + 0
    ),
    divide: (
      (left, right) => Math.trunc(left / right) + 0
    ),
    remainder: (
      (left, right) => left % right + 0
    ),
    negate: (
      value => -value + 0
    ),
    abs: (
      Math.abs
    ),
    ordering_less_than: (
      (left, right) => left < right
    ),
    ordering_greater_than: (
      (left, right) => left > right
    ),
    min: (
      Math.min
    ),
    max: (
      Math.max
    ),
    clamp: (
      (value, minimum, maximum) => Math.min(Math.max(value, minimum), maximum)
    ),
    from_nat: (
      value => value
    ),
    from_real: (
      value => Math.trunc(value) + 0
    ),
    add8: (
      (left, right) => ((left + right) << 24 >> 24)
    ),
    subtract8: (
      (left, right) => ((left - right) << 24 >> 24)
    ),
    multiply8: (
      (left, right) => ((left * right) << 24 >> 24)
    ),
    divide8: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((Math.trunc(left / right)) << 24 >> 24); }
    ),
    remainder8: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((left % right) << 24 >> 24); }
    ),
    negate8: (
      value => ((-value) << 24 >> 24)
    ),
    abs8: (
      value => value < 0 ? ((-value) << 24 >> 24) : value
    ),
    ordering_less_than8: (
      (left, right) => left < right
    ),
    ordering_greater_than8: (
      (left, right) => left > right
    ),
    min8: (
      (left, right) => left < right ? left : right
    ),
    max8: (
      (left, right) => left > right ? left : right
    ),
    clamp8: (
      (value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value
    ),
    is_zero8: (
      value => value === 0
    ),
    from_nat8: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) << 24 >> 24); }
    ),
    from_int8: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) << 24 >> 24); }
    ),
    to_string8: (
      value => String(value)
    ),
    to_int8: (
      value => { const result = value; if (!Number.isSafeInteger(result)) throw new RangeError("integer out of range"); return result; }
    ),
    add16: (
      (left, right) => ((left + right) << 16 >> 16)
    ),
    subtract16: (
      (left, right) => ((left - right) << 16 >> 16)
    ),
    multiply16: (
      (left, right) => ((left * right) << 16 >> 16)
    ),
    divide16: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((Math.trunc(left / right)) << 16 >> 16); }
    ),
    remainder16: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((left % right) << 16 >> 16); }
    ),
    negate16: (
      value => ((-value) << 16 >> 16)
    ),
    abs16: (
      value => value < 0 ? ((-value) << 16 >> 16) : value
    ),
    ordering_less_than16: (
      (left, right) => left < right
    ),
    ordering_greater_than16: (
      (left, right) => left > right
    ),
    min16: (
      (left, right) => left < right ? left : right
    ),
    max16: (
      (left, right) => left > right ? left : right
    ),
    clamp16: (
      (value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value
    ),
    is_zero16: (
      value => value === 0
    ),
    from_nat16: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) << 16 >> 16); }
    ),
    from_int16: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) << 16 >> 16); }
    ),
    to_string16: (
      value => String(value)
    ),
    to_int16: (
      value => { const result = value; if (!Number.isSafeInteger(result)) throw new RangeError("integer out of range"); return result; }
    ),
    add32: (
      (left, right) => ((left + right) | 0)
    ),
    subtract32: (
      (left, right) => ((left - right) | 0)
    ),
    multiply32: (
      (left, right) => ((Math.imul(left, right)) | 0)
    ),
    divide32: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((Math.trunc(left / right)) | 0); }
    ),
    remainder32: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((left % right) | 0); }
    ),
    negate32: (
      value => ((-value) | 0)
    ),
    abs32: (
      value => value < 0 ? ((-value) | 0) : value
    ),
    ordering_less_than32: (
      (left, right) => left < right
    ),
    ordering_greater_than32: (
      (left, right) => left > right
    ),
    min32: (
      (left, right) => left < right ? left : right
    ),
    max32: (
      (left, right) => left > right ? left : right
    ),
    clamp32: (
      (value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value
    ),
    is_zero32: (
      value => value === 0
    ),
    from_nat32: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) | 0); }
    ),
    from_int32: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) | 0); }
    ),
    to_string32: (
      value => String(value)
    ),
    to_int32: (
      value => { const result = value; if (!Number.isSafeInteger(result)) throw new RangeError("integer out of range"); return result; }
    ),
    add64: (
      (left, right) => BigInt.asIntN(64, left + right)
    ),
    subtract64: (
      (left, right) => BigInt.asIntN(64, left - right)
    ),
    multiply64: (
      (left, right) => BigInt.asIntN(64, left * right)
    ),
    divide64: (
      (left, right) => BigInt.asIntN(64, left / right)
    ),
    remainder64: (
      (left, right) => BigInt.asIntN(64, left % right)
    ),
    negate64: (
      value => BigInt.asIntN(64, -value)
    ),
    abs64: (
      value => value < 0n ? BigInt.asIntN(64, -value) : value
    ),
    ordering_less_than64: (
      (left, right) => left < right
    ),
    ordering_greater_than64: (
      (left, right) => left > right
    ),
    min64: (
      (left, right) => left < right ? left : right
    ),
    max64: (
      (left, right) => left > right ? left : right
    ),
    clamp64: (
      (value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value
    ),
    is_zero64: (
      value => value === 0n
    ),
    from_nat64: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return BigInt.asIntN(64, BigInt(value)); }
    ),
    from_int64: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return BigInt.asIntN(64, BigInt(value)); }
    ),
    to_string64: (
      value => String(value)
    ),
    to_int64: (
      value => { const result = Number(value); if (!Number.isSafeInteger(result)) throw new RangeError("integer out of range"); return result; }
    ),
  },
  json: {
    char_code: (
      text => text.charCodeAt(0)
    ),
    char_from_code: (
      String.fromCodePoint
    ),
    quote: (
      JSON.stringify
    ),
    real_value: (
      Number
    ),
    real_token: (
      value => Object.is(value, -0) ? "-0.0" : String(value)
    ),
    nat_of_digits: (
      Number
    ),
    int_of_digits: (
      Number
    ),
    nat64_of_digits: (
      BigInt
    ),
    int64_of_digits: (
      BigInt
    ),
  },
  nat: {
    add: (
      (left, right) => left + right
    ),
    subtract: (
      (left, right) => Math.max(0, left - right)
    ),
    multiply: (
      (left, right) => left * right
    ),
    divide: (
      (left, right) => Math.trunc(left / right)
    ),
    remainder: (
      (left, right) => left % right
    ),
    ordering_less_than: (
      (left, right) => left < right
    ),
    ordering_greater_than: (
      (left, right) => left > right
    ),
    min: (
      Math.min
    ),
    max: (
      Math.max
    ),
    clamp: (
      (value, minimum, maximum) => Math.min(Math.max(value, minimum), maximum)
    ),
    from_int: (
      value => Math.max(0, value)
    ),
    from_real: (
      value => Math.max(0, Math.trunc(value))
    ),
    add8: (
      (left, right) => ((left + right) & 255)
    ),
    subtract8: (
      (left, right) => ((left - right) & 255)
    ),
    multiply8: (
      (left, right) => ((left * right) & 255)
    ),
    divide8: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((Math.trunc(left / right)) & 255); }
    ),
    remainder8: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((left % right) & 255); }
    ),
    ordering_less_than8: (
      (left, right) => left < right
    ),
    ordering_greater_than8: (
      (left, right) => left > right
    ),
    min8: (
      (left, right) => left < right ? left : right
    ),
    max8: (
      (left, right) => left > right ? left : right
    ),
    clamp8: (
      (value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value
    ),
    is_zero8: (
      value => value === 0
    ),
    from_nat8: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) & 255); }
    ),
    from_int8: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) & 255); }
    ),
    to_string8: (
      value => String(value)
    ),
    to_nat8: (
      value => { const result = value; if (!Number.isSafeInteger(result) || result < 0) throw new RangeError("integer out of range"); return result; }
    ),
    add16: (
      (left, right) => ((left + right) & 65535)
    ),
    subtract16: (
      (left, right) => ((left - right) & 65535)
    ),
    multiply16: (
      (left, right) => ((left * right) & 65535)
    ),
    divide16: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((Math.trunc(left / right)) & 65535); }
    ),
    remainder16: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((left % right) & 65535); }
    ),
    ordering_less_than16: (
      (left, right) => left < right
    ),
    ordering_greater_than16: (
      (left, right) => left > right
    ),
    min16: (
      (left, right) => left < right ? left : right
    ),
    max16: (
      (left, right) => left > right ? left : right
    ),
    clamp16: (
      (value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value
    ),
    is_zero16: (
      value => value === 0
    ),
    from_nat16: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) & 65535); }
    ),
    from_int16: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) & 65535); }
    ),
    to_string16: (
      value => String(value)
    ),
    to_nat16: (
      value => { const result = value; if (!Number.isSafeInteger(result) || result < 0) throw new RangeError("integer out of range"); return result; }
    ),
    add32: (
      (left, right) => ((left + right) >>> 0)
    ),
    subtract32: (
      (left, right) => ((left - right) >>> 0)
    ),
    multiply32: (
      (left, right) => ((Math.imul(left, right)) >>> 0)
    ),
    divide32: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((Math.trunc(left / right)) >>> 0); }
    ),
    remainder32: (
      (left, right) => { if (right === 0) throw new RangeError("division by zero"); return ((left % right) >>> 0); }
    ),
    ordering_less_than32: (
      (left, right) => left < right
    ),
    ordering_greater_than32: (
      (left, right) => left > right
    ),
    min32: (
      (left, right) => left < right ? left : right
    ),
    max32: (
      (left, right) => left > right ? left : right
    ),
    clamp32: (
      (value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value
    ),
    is_zero32: (
      value => value === 0
    ),
    from_nat32: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) >>> 0); }
    ),
    from_int32: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return ((value) >>> 0); }
    ),
    to_string32: (
      value => String(value)
    ),
    to_nat32: (
      value => { const result = value; if (!Number.isSafeInteger(result) || result < 0) throw new RangeError("integer out of range"); return result; }
    ),
    add64: (
      (left, right) => BigInt.asUintN(64, left + right)
    ),
    subtract64: (
      (left, right) => BigInt.asUintN(64, left - right)
    ),
    multiply64: (
      (left, right) => BigInt.asUintN(64, left * right)
    ),
    divide64: (
      (left, right) => BigInt.asUintN(64, left / right)
    ),
    remainder64: (
      (left, right) => BigInt.asUintN(64, left % right)
    ),
    ordering_less_than64: (
      (left, right) => left < right
    ),
    ordering_greater_than64: (
      (left, right) => left > right
    ),
    min64: (
      (left, right) => left < right ? left : right
    ),
    max64: (
      (left, right) => left > right ? left : right
    ),
    clamp64: (
      (value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value
    ),
    is_zero64: (
      value => value === 0n
    ),
    from_nat64: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return BigInt.asUintN(64, BigInt(value)); }
    ),
    from_int64: (
      value => { if (!Number.isSafeInteger(value)) throw new RangeError("expected a safe integer"); return BigInt.asUintN(64, BigInt(value)); }
    ),
    to_string64: (
      value => String(value)
    ),
    to_nat64: (
      value => { const result = Number(value); if (!Number.isSafeInteger(result) || result < 0) throw new RangeError("integer out of range"); return result; }
    ),
  },
  real: {
    remainder: (
      (left, right) => left % right
    ),
    abs: (
      Math.abs
    ),
    sqrt: (
      Math.sqrt
    ),
    power: (
      Math.pow
    ),
    ordering_equal: (
      Object.is
    ),
    ordering_less_than: (
      (left, right) => left < right
    ),
    ordering_greater_than: (
      (left, right) => left > right
    ),
    min: (
      Math.min
    ),
    max: (
      Math.max
    ),
    clamp: (
      (value, minimum, maximum) => Math.min(Math.max(value, minimum), maximum)
    ),
    floor: (
      Math.floor
    ),
    ceil: (
      Math.ceil
    ),
    round: (
      Math.round
    ),
    truncate: (
      Math.trunc
    ),
    is_nan: (
      Number.isNaN
    ),
    is_finite: (
      Number.isFinite
    ),
    is_integer: (
      Number.isInteger
    ),
    from_nat: (
      value => value
    ),
    from_int: (
      value => value
    ),
    sin: (
      Math.sin
    ),
    cos: (
      Math.cos
    ),
    tan: (
      Math.tan
    ),
    asin: (
      Math.asin
    ),
    acos: (
      Math.acos
    ),
    atan: (
      Math.atan
    ),
    atan2: (
      Math.atan2
    ),
    sinh: (
      Math.sinh
    ),
    cosh: (
      Math.cosh
    ),
    tanh: (
      Math.tanh
    ),
    asinh: (
      Math.asinh
    ),
    acosh: (
      Math.acosh
    ),
    atanh: (
      Math.atanh
    ),
    exp: (
      Math.exp
    ),
    expm1: (
      Math.expm1
    ),
    log: (
      Math.log
    ),
    log1p: (
      Math.log1p
    ),
    log2: (
      Math.log2
    ),
    log10: (
      Math.log10
    ),
  },
  str: {
    concat: (
      (left, right) => left + right
    ),
    len: (
      value => { let count = 0; for (const _ of value) count++; return count; }
    ),
    from_nat: (
      value => String(value)
    ),
    from_int: (
      value => String(value)
    ),
    from_real: (
      value => String(value)
    ),
    ordering_less_than: (
      (left, right) => { const a = Array.from(left, c => c.codePointAt(0)); const b = Array.from(right, c => c.codePointAt(0)); for (let i = 0; i < a.length && i < b.length; i++) { if (a[i] !== b[i]) return a[i] < b[i]; } return a.length < b.length; }
    ),
    ordering_greater_than: (
      (left, right) => { const a = Array.from(left, c => c.codePointAt(0)); const b = Array.from(right, c => c.codePointAt(0)); for (let i = 0; i < a.length && i < b.length; i++) { if (a[i] !== b[i]) return a[i] > b[i]; } return a.length > b.length; }
    ),
    contains: (
      (value, search) => value.includes(search)
    ),
    starts_with: (
      (value, prefix) => value.startsWith(prefix)
    ),
    ends_with: (
      (value, suffix) => value.endsWith(suffix)
    ),
    char_at: (
      (value, index) => { let at = 0; for (const c of value) { if (at === index) return c; at++; } return ""; }
    ),
    index_of: (
      (value, search) => { const at = value.indexOf(search); if (at < 0) return -1; let count = 0; for (let i = 0; i < at; i++) { const code = value.charCodeAt(i); if (code < 0xdc00 || code > 0xdfff) count++; } return count; }
    ),
    slice: (
      (value, start, end) => Array.from(value).slice(start, end).join("")
    ),
    trim: (
      value => value.trim()
    ),
    trim_start: (
      value => value.trimStart()
    ),
    trim_end: (
      value => value.trimEnd()
    ),
    to_lowercase: (
      value => value.toLowerCase()
    ),
    to_uppercase: (
      value => value.toUpperCase()
    ),
    repeat: (
      (value, count) => value.repeat(count)
    ),
    replace_first: (
      (value, search, replacement) => value.replace(search, replacement)
    ),
    replace_all: (
      (value, search, replacement) => value.replaceAll(search, replacement)
    ),
    pad_start: (
      (value, length, fill) => { const units = Array.from(fill); let count = Array.from(value).length; let pad = ""; for (let i = 0; units.length && count < length; i++, count++) pad += units[i % units.length]; return pad + value; }
    ),
    pad_end: (
      (value, length, fill) => { const units = Array.from(fill); let count = Array.from(value).length; let pad = ""; for (let i = 0; units.length && count < length; i++, count++) pad += units[i % units.length]; return value + pad; }
    ),
    reverse: (
      value => Array.from(value).reverse().join("")
    ),
  },
};
