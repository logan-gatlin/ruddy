//! Tests for [`esparse`]: what it accepts, what it refuses, that it agrees
//! with oxc on both, and that depth costs it memory rather than stack.

use esparse::{Item, parse_module};

/// Modules the grammar admits, one construct or edge each.
const VALID: &[&str] = &[
    "",
    "#!/usr/bin/env node\nlet x = 1;",
    "let x = 1; const y = 2, z = 3; var w;",
    "let [a, , b = 2, ...rest] = xs; const { c, d: e, f = 1, ...g } = o;",
    "function f(a, b = 1, ...c) { return a + b; }",
    "function* gen() { yield; yield 1; yield* other(); const x = yield; }",
    "async function af() { await x; for await (const y of z) {} }",
    "const arrow = (a, b) => a + b; const one = x => x * 2; const none = () => {};",
    "const aa = async (a) => await a; const ab = async a => a; const ac = async () => {};",
    "const call = async(1, 2); const call3 = f(a, ...b,);",
    "const obj = { a, b: 1, 'c': 2, 3: 4, [k]: 5, m() {}, get g() { return 1; }, set s(v) {}, async am() {}, *gm() {}, async *agm() {}, ...spread, get: 1, set, async, static: 2 };",
    "const arr = [1, , 2, ...xs, ];",
    "class A extends B { static x = 1; y; #p = 2; static #q; constructor() { super(); } m() { return super.m(); } get g() {} set g(v) {} static async *s() {} static { init(); } #priv() {} static m2() {} ['computed']() {} 'quoted'() {} 42() {} }",
    "class C { static; get; set; async; static = 1; async = 2; get = 3; }",
    "const c = class Named {}; const d = class {};",
    "if (a) b; else if (c) d; else { e; }",
    "for (let i = 0; i < 10; i++) {} for (;;) break; for (const k in o) {} for (x of xs) {} for ([a, b] of pairs) {} for ({ a } of objs) {} for (let [k, v] of m) {}",
    "for (var i = 0, j = 1; i < j; i++, j--) ;",
    "while (x) { continue; } do x++; while (y) do {} while (z);",
    "outer: for (;;) { inner: for (;;) { break outer; continue inner; } }",
    "switch (x) { case 1: case 2: f(); break; default: g(); case 3: {} }",
    "try { f(); } catch (e) { g(); } finally { h(); } try {} catch {} try {} finally {}",
    "try {} catch ({ message }) {} try {} catch ([first]) {}",
    "throw new Error('x');",
    "debugger;",
    "a = b; a += 1; a **= 2; a ??= 3; a ||= 4; a &&= 5; a.b = 1; a[b] = 2; [a, b] = [b, a]; ({ a, b } = o); [a.b, c[d]] = xs; ({ x: a.b } = o);",
    "x = a ? b : c; x = a ?? b; x = (a || b) ?? c; x = a || b && c; x = a ** b ** c; x = (-a) ** b; x = a++ ** 2;",
    "x = typeof a; x = void 0; x = delete a.b; x = !a; x = -a; x = +a; x = ~a; x = ++a; x = --a; x = a++; x = a--;",
    "x = a.b.c; x = a[b][c]; x = a?.b; x = a?.[b]; x = a?.(); x = a?.b.c(); x = a?.b?.c; x = new A; x = new A(); x = new A.B(); x = new new A()(); x = new (f())(); x = import.meta; function nt() { return new.target; }",
    "x = f`tag`; x = f`a${b}c${d}e`; x = `plain`; x = `${a}${b}`; x = `nested ${`inner ${deep}`}`; x = `}`; x = `\\u{1F600} \\x41 \\n`;",
    "x = /re/g; x = /[/]/; x = /a\\/b/dgimsuy; x = /a/v; x = a / b / c; x = a /= 2; x = (1) / 2; x = [] / 2;",
    "x = { a = 1 } = o; [{ a = 1 }] = xs; ({ a = 1 }) => a;",
    "x = 1_000; x = 0xff_ff; x = 0b1010n; x = 0o17; x = .5; x = 5.; x = 1e10; x = 1E-5; x = 1n; x = 0;",
    "x = 'it\\'s'; x = \"q\\\"q\"; x = 'line\\\ncontinued'; x = '\\0'; x = '\\u{10FFFF}'; x = '\\u00e9';",
    "x = a, b, c; x = (a, b); f((a, b), c);",
    "x = a\n++b",
    "x = a\n(b)",
    "return_ = 1\nlet y = 2\nvar z\nz = 3",
    "x = function () {}(); x = function named() {}; x = (function () {})();",
    "x = { if: 1, class: 2, in: 3, 'a b': 4, await: 5 }; x = a.if; x = a.class; x = a.await;",
    "x = a in b; x = a instanceof b; for (let i = (a in b); ;) ;",
    "x = a => b => c; x = async a => async b => c;",
    "x = (a) => ({}); x = () => [];",
    "x = async function () {}; x = async function* () {}; x = function* () {};",
    "x = () => { return; }; x = () => { return 1 }",
    "x = a ? (b) : c => d;",
    "x = a?.5:b;",
    "x = ((a));",
    "x = [ , ];",
    "x = {}; ({});",
    "import 'side-effect'; import def from 'a'; import * as ns from 'b'; import { a, b as c, 'string name' as d } from 'e'; import def2, { x } from 'f'; import def3, * as ns2 from 'g'; import json from './x.json' with { type: 'json' };",
    "export const a = 1; export let b; export var c = 2; export function f() {} export async function g() {} export class K {} export function* h() {}",
    "export { a, b as c, d as 'string name' }; export { x } from 'y'; export * from 'z'; export * as ns from 'w'; export {};",
    "export default 1;",
    "export default function () {}",
    "export default function named() {}",
    "export default async function () {}",
    "export default class {}",
    "export default class Named {}",
    "export default (a, b) => a + b;",
    "export default async () => {};",
    "x = import('mod'); x = import('mod', { with: {} }); x = await import('m');",
    "await x; for await (const y of z) {}",
    "label: { break label; }",
    "x = a\n/b/g",
    "x = class { #a; has(o) { return #a in o; } }",
    "x = { __proto__: null, 'use strict': 1 };",
    "x = a ?? b ?? c; x = a || b || c;",
    "x = async\n(y)",
    "x = async;",
    "x = yield_;",
    "let async = 1; let of = 2; let get = 3; let set = 4; let from = 5; let as = 6; let target = 7; let meta = 8;",
    "// comment\n/* multi\nline */ x /* inline */ = 1 // trailing",
    "// ── boxed ──\nx = 1; /* ═ */ y = 'é'; z = `─${a}─`; w = /─/; // ✓\nv = 2;",
    "x = a\n/* newline in comment */ ++b",
    "x = \u{FEFF}1; x\u{00A0}= 2; x\u{2028}= 3;",
    "x = ünïcödé; x = \\u0041bc; x = a\\u{62}c;",
    "x = { async *[Symbol.iterator]() {} };",
    "x = { get [k]() {}, set [k](v) {} };",
    "for (let of = 1; ;) break; for (const x of async) {}",
    "x = a ? b ? c : d : e;",
    "x = (a, b) => (c, d) => e;",
    "for (const [k, v = 1] of [[1]]) ;",
    "x = a.#b; class D { #b; f(o) { return o.#b; } }",
    "x = 1; ;;;",
    "x = { ...a.b } = c; ({ ...a[b] } = c);",
    "x = (a) => ({}) ();",
    "class A { async\nfoo() {} }",
    "class A { static\nfoo() {} get\nbar() {} }",
    "label: label2",
];

/// Modules the grammar refuses, one reason each.
const INVALID: &[&str] = &[
    "let",
    "let x = ;",
    "const x;",
    "let [a];",
    "x = ;",
    "x = (;",
    "x = );",
    "x = [1, 2;",
    "x = { a: };",
    "x = { a = 1 };",
    "({ a = 1 });",
    "[{ a = 1 }];",
    "x = { a b };",
    "function () {}",
    "function f( { }",
    "class A { foo bar }",
    "if (a) else b;",
    "for (;;",
    "for (const x of y, z) ;",
    "for (let x, y of z) ;",
    "for (const x;;) ;",
    "while x {}",
    "do x while (y)",
    "return 1;",
    "throw\nnew Error()",
    "break foo bar;",
    "try {}",
    "try {} catch",
    "switch (x) { x }",
    "1 = 2;",
    "a + b = c;",
    "(a, b) = c;",
    "({ a }) = c;",
    "[a.b] => 1;",
    "(a.b) => 1;",
    "(...a, b) => 1;",
    "(...a,) => 1;",
    "(1) => 1;",
    "() => 1 = 2;",
    "x = () => {} ();",
    "x = () => {} + 1;",
    "x = a\n=> b;",
    "x = async a\n=> b;",
    "x = -a ** b;",
    "x = a ?? b || c;",
    "x = a || b ?? c;",
    "x = ++a++;",
    "x = 1++;",
    "x = a?.`tpl`;",
    "x = a?.b = 1;",
    "x = new a?.b();",
    "x = super;",
    "x = import;",
    "x = import.foo;",
    "x = new.foo;",
    "x = 1_;",
    "x = 1__0;",
    "x = 0x;",
    "x = 1e;",
    "x = 1.5n;",
    "x = 3in y;",
    "x = 'unterminated",
    "x = 'line\nbreak';",
    "x = '\\x4';",
    "x = '\\u{110000}';",
    "x = `unterminated",
    "x = `${a`;",
    "x = /unterminated",
    "x = /a/gg;",
    "x = /a/q;",
    "x = /a/uv;",
    "x = /a\nb/;",
    "x = /*unterminated",
    "x = #priv;",
    "x = { #a: 1 };",
    "x = a.;",
    "x = a.1;",
    "x = a?.;",
    "x = @;",
    "x = \\u0000;",
    "x = a\\u0020b;",
    "x = { get a };",
    "x = { async a };",
    "x = { *a };",
    "x = { a() };",
    "class A { get x }",
    "class A extends {}",
    "let await = 1;",
    "let enum = 1;",
    "x = { await };",
    "function f() { await x; }",
    "function f() { for await (const x of y) {} }",
    "function f() { yield 1; }",
    "yield 1;",
    "x = a => { return }; return;",
    "import { a as } from 'b';",
    "import { 'x' } from 'b';",
    "import * from 'b';",
    "import a, from 'b';",
    "import 'a' with { type: json };",
    "export;",
    "export 1;",
    "export * as from 'b';",
    "export default;",
    "export default let x = 1;",
    "export function () {}",
    "x = { a: 1,, };",
    "x = [a,,] = b; x = [...a,] = b;",
    "x = { ...a, b } = c;",
    "[...a.b] => 1;",
    "async function f() { await => 1; }",
    "x = async (a, a.b) => 1;",
    "x = 1 ? 2;",
    "x = 1 ? 2 : ;",
    "x = a?.5;",
    "for (a of b of c) ;",
    "for await (x in y) {}",
    "for await (let x = 1; ;) {}",
    "for await (; ;) {}",
    "x = { async\nfoo() {} };",
    "x = async\nfunction () {};",
    "x = { 'a' };",
    "x = { 1 };",
    "x = { [a] };",
    "if (a) let x = 1;",
    "while (a) const x = 1;",
    "l: class A {}",
    "for (;;) async function f() {}",
    "x = a\n?.b\n= 1;",
    "x =",
    "{",
    "}",
    "x = {",
    "x = [",
    "x = (",
    "x = f(",
    "x = `${",
    "switch (x) {",
    "class A {",
    "function f() {",
    "if (",
    "for (",
    "x = [] = [",
];

/// Modules this recognizer refuses for strict-mode and early-error rules
/// that oxc's parser leaves to its semantic pass, so the oracle cannot vouch
/// for them.
const INVALID_STRICT: &[&str] = &[
    "if (a) function f() {}",
    "l: function f() {}",
    "with (o) {}",
    "x = delete a;",
    "x = 0777;",
    "x = 08;",
    "x = '\\1';",
    "let let = 1;",
    "let static = 1;",
    "let yield = 1;",
    "let eval = 1;",
    "let arguments = 1;",
    "x = { let };",
    "x = { yield };",
    "import x from 'y'; function f() { import z from 'w'; }",
    "function f() { export const x = 1; }",
    "export { a } from;",
    "export class {}",
];

/// A deeply nested prefix, its element, and its closing suffix.
fn nested(open: &str, inner: &str, close: &str, depth: usize) -> String {
    let mut source = String::with_capacity(depth * (open.len() + close.len()) + inner.len());
    for _ in 0..depth {
        source.push_str(open);
    }
    source.push_str(inner);
    for _ in 0..depth {
        source.push_str(close);
    }
    source
}

/// Whether oxc reads `source` as a module without complaint.
fn oxc_accepts(source: &str) -> bool {
    let allocator = oxc_allocator::Allocator::default();
    let parsed = oxc_parser::Parser::new(&allocator, source, oxc_span::SourceType::mjs()).parse();
    !parsed.panicked && !parsed.diagnostics.has_errors()
}

#[test]
fn the_valid_corpus_parses() {
    for source in VALID {
        if let Err(error) = parse_module(source) {
            panic!("refused {source:?}: {error}");
        }
    }
}

#[test]
fn the_invalid_corpus_is_refused() {
    for source in INVALID.iter().chain(INVALID_STRICT) {
        assert!(parse_module(source).is_err(), "accepted {source:?}");
    }
}

/// The recognizer agrees with oxc on every line of both corpora, which is
/// what makes them a fair sample rather than a description of this parser.
#[test]
fn oxc_agrees_with_both_corpora() {
    let mut disagreements = Vec::new();
    for source in VALID {
        if !oxc_accepts(source) {
            disagreements.push(format!("oxc refuses the valid {source:?}"));
        }
    }
    for source in INVALID {
        if oxc_accepts(source) {
            disagreements.push(format!("oxc accepts the invalid {source:?}"));
        }
    }
    assert!(disagreements.is_empty(), "{}", disagreements.join("\n"));
}

#[test]
fn errors_say_where() {
    let error = parse_module("let x = 1;\nlet y = ;").unwrap_err();
    assert_eq!(error.position(), (2, 9));
    assert_eq!(error.offset(), 19);
    assert_eq!(
        error.to_string(),
        "expected an expression at line 2, column 9"
    );
}

#[test]
fn top_level_items_are_reported() {
    let module = parse_module(
        "import a from 'b';\nexport default (x);\nexport { a };\nlet y;\nimport('c');",
    )
    .unwrap();
    assert_eq!(
        module.items(),
        [
            Item::Import,
            Item::ExportDefault,
            Item::Export,
            Item::Statement,
            Item::Statement,
        ]
    );
}

/// Depth is paid for in heap. A recursive-descent parser overflows a small
/// stack thousands of levels before these; the recognizer is run on one far
/// smaller than any thread the backend is called from.
#[test]
fn nesting_costs_memory_rather_than_stack() {
    const DEPTH: usize = 50_000;
    let expressions = [
        nested("[", "1", "]", DEPTH),
        nested("(", "1", ")", DEPTH),
        nested("{ a: ", "1", " }", DEPTH),
        nested("f(", "1", ")", DEPTH),
        nested("() => ", "1", "", DEPTH),
        nested("- ", "1", "", DEPTH),
        nested("a + ", "1", "", DEPTH),
        nested("a ? ", "1", " : 0", DEPTH),
        nested("`${", "1", "}`", DEPTH),
        nested("[", "x", "]", DEPTH) + " = y",
        nested("(", "x", ")", DEPTH) + " = y",
        nested("new ", "X", "", DEPTH),
        nested("a[", "1", "]", DEPTH),
    ];
    let statements = [
        nested("{ ", "", " }", DEPTH),
        nested("if (a) ", "x;", "", DEPTH),
        nested("while (a) ", "x;", "", DEPTH),
        nested("l: ", "x;", "", DEPTH),
        nested("function f() { ", "", " }", DEPTH),
        nested("() => { ", "", " }", DEPTH),
        nested("try { ", "", " } finally {}", DEPTH),
        nested("switch (a) { case 1: ", "", " }", DEPTH),
        nested("class A { m() { ", "", " } }", DEPTH),
        format!("let {} = x;", nested("[", "a", "]", DEPTH)),
        format!("let {} = x;", nested("{ a: ", "b", " }", DEPTH)),
    ];
    std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || {
            for source in &expressions {
                let source = format!("x = {source};");
                parse_module(&source).unwrap_or_else(|error| panic!("{error}: {source:.40}"));
            }
            for source in &statements {
                parse_module(source).unwrap_or_else(|error| panic!("{error}: {source:.40}"));
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
