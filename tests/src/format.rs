//! Tests for [`ruddy::format`], the source formatter behind `ruddy fmt`.

use ruddy::{
    format::{Formatted, format},
    parse, token,
    tracking::FileID,
};
use ruddy_debug::print;

fn formatted(source: &str) -> Formatted {
    format(source, FileID::GENERATED)
}

/// Format a source that has no errors, and check the result is a fixed point
/// that re-parses to the tree it was printed from.
fn fmt(source: &str) -> String {
    let out = formatted(source);
    assert!(
        !out.has_errors(),
        "{source:?}: {:#?} {:#?}",
        out.lex_errors,
        out.parse_errors
    );
    assert_eq!(
        printed_tree(&out.text),
        printed_tree(source),
        "formatting {source:?} changed the tree:\n{}",
        out.text
    );
    let again = formatted(&out.text);
    assert_eq!(
        again.text, out.text,
        "formatting is not a fixed point for {source:?}"
    );
    out.text
}

/// The parse tree of a source as the debugger prints it, one statement per
/// line: what a formatter must leave untouched.
fn printed_tree(source: &str) -> String {
    let lexed = token::lex(source, FileID::GENERATED);
    let parsed = parse::parse(lexed.tokens);
    assert!(
        lexed.errors.is_empty() && parsed.errors.is_empty(),
        "{source:?}: {:#?} {:#?}",
        lexed.errors,
        parsed.errors
    );
    parsed
        .stmts
        .iter()
        .map(|stmt| print::ast::stmt(stmt).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_definition_is_printed_on_one_line_when_it_fits() {
    assert_eq!(fmt("let   x   =   1n"), "let x = 1n\n");
    assert_eq!(fmt("let x : Nat = 1n"), "let x: Nat = 1n\n");
    assert_eq!(fmt("let f = fn a b => a"), "let f = fn a b => a\n");
    assert_eq!(fmt(""), "");
    assert_eq!(fmt("\n\n"), "");
}

#[test]
fn discard_bindings_preserve_their_written_form() {
    assert_eq!(fmt("_  =  tick ()"), "_ = tick ()\n");
    assert_eq!(fmt("_ : Nat = 1n"), "_: Nat = 1n\n");
    assert_eq!(fmt("let  _  =  tick ()"), "let _ = tick ()\n");
    assert_eq!(fmt("let _ : Nat = 1n"), "let _: Nat = 1n\n");
    assert_eq!(
        fmt("_=first () _=second () let _=third ()"),
        "_ = first ()\n_ = second ()\nlet _ = third ()\n"
    );
    assert_eq!(
        fmt("module M = _=first () let _=second () end"),
        "module M = _ = first () let _ = second () end\n"
    );
    assert_eq!(
        fmt("let result = do _=first () let _=second () return 3n end"),
        "let result = do _ = first () let _ = second () return 3n end\n"
    );
    assert_eq!(
        fmt("_=do\n_=first ()\nlet _=second ()\nend"),
        "_ = do\n  _ = first ()\n  let _ = second ()\nend\n"
    );
}

#[test]
fn discard_binding_attributes_keep_their_comments() {
    assert_eq!(fmt("@a @b _=tick ()"), "@a\n@b\n_ = tick ()\n");
    let source = "@doc \"effects\"\n-- about the discard\n_ = tick ()\nmodule M =\n  @a\n  (* before the discard *)\n  _ = tock ()\nend\n";
    assert_eq!(fmt(source), source);
}

#[test]
fn discard_binding_comments_survive_formatting() {
    assert_eq!(
        fmt("-- before\n_ = tick () -- after\n"),
        "-- before\n_ = tick () -- after\n"
    );
    assert_eq!(
        fmt("_ (* between *) = (* body *) tick ()"),
        "_ (* between *) = (* body *) tick ()\n"
    );
    assert_eq!(
        fmt("_ -- between\n= tick ()"),
        "_ = -- between\n  tick ()\n"
    );
    assert_eq!(
        fmt("_ (* pattern *) : Nat (* type *) = 1n"),
        "_ (* pattern *): Nat (* type *) = 1n\n"
    );
    let source = "let result = do\n  -- before\n  _ =\n    -- body\n    tick ()\n  -- after\nend\n";
    assert_eq!(fmt(source), source);
}

#[test]
fn blank_lines_between_definitions_are_kept_but_collapsed() {
    assert_eq!(
        fmt("let a = 1n\n\n\n\nlet b = 2n\nlet c = 3n\n"),
        "let a = 1n\n\nlet b = 2n\nlet c = 3n\n"
    );
}

#[test]
fn match_arms_sit_at_the_column_of_the_match_line() {
    let source = "let f = fn v => match v with\n  | #Some x => x\n  | #None => 0n\nend\n";
    assert_eq!(
        fmt(source),
        "let f = fn v => match v with\n| #Some x => x\n| #None => 0n\nend\n"
    );
    // The author's one-line match stays one while it fits.
    assert_eq!(
        fmt("let f = fn v => match v with | #Some x => x | #None => 0n end"),
        "let f = fn v => match v with | #Some x => x | #None => 0n end\n"
    );
}

#[test]
fn comments_keep_their_place() {
    let source = "-- leading\nlet a = 1n -- trailing\n\n-- between\n\nlet b = 2n\n-- last\n";
    assert_eq!(fmt(source), source);
    assert_eq!(
        fmt("let s = {\n  -- about x\n  x: 1n, -- after x\n  y: 2n\n  -- before the brace\n}\n"),
        "let s = {\n  -- about x\n  x: 1n, -- after x\n  y: 2n,\n  -- before the brace\n}\n"
    );
}

/// Every Ruddy source in the repository formats to a fixed point that
/// re-parses to the tree it came from. The backend's handlers are fragments
/// spliced into generated code rather than files, so they are left out; the demo ends in deliberately broken definitions, so its errors are
/// allowed and its surviving statements compared.
#[test]
fn the_repository_sources_round_trip() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for directory in ["std", "std/path", "src/backend"] {
        for entry in std::fs::read_dir(root.join(directory)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "rud")
                && !path.ends_with("node-handler.rud")
                && !path.ends_with("web-handler.rud")
            {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(files.len() > 10);
    for path in files {
        let source = std::fs::read_to_string(&path).unwrap();
        let text = fmt(&source);
        for line in text.lines() {
            assert!(
                line.chars().count() <= ruddy::format::WIDTH || line.contains('"'),
                "{}: line too long: {line}",
                path.display()
            );
        }
    }
    let demo = std::fs::read_to_string(root.join("demo.rud")).unwrap();
    let out = formatted(&demo);
    assert!(out.has_errors(), "the demo's broken tail is under test");
    assert_eq!(surviving_tree(&out.text), surviving_tree(&demo));
    assert_eq!(formatted(&out.text).text, out.text);
}

/// The statements a source with errors still parses to, printed.
fn surviving_tree(source: &str) -> String {
    let lexed = token::lex(source, FileID::GENERATED);
    let parsed = parse::parse(lexed.tokens);
    parsed
        .stmts
        .iter()
        .map(|stmt| print::ast::stmt(stmt).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format a source and check only that the result is a fixed point: for
/// sources with errors, whose tree is not the whole of their text.
fn fmt_lossy(source: &str) -> String {
    let out = formatted(source);
    assert_eq!(
        formatted(&out.text).text,
        out.text,
        "formatting is not a fixed point for {source:?}"
    );
    out.text
}

#[test]
fn comments_attach_to_the_node_beside_them() {
    // Inline before a node, after a node, and after an operand of a chain.
    assert_eq!(fmt("let x = (* inline *) 1n"), "let x = (* inline *) 1n\n");
    assert_eq!(fmt("let y = 1n (* after *)"), "let y = 1n (* after *)\n");
    assert_eq!(
        fmt("let z = f a -- after a\n  b\n"),
        "let z = f\n  a -- after a\n  b\n"
    );
    assert_eq!(
        fmt("let p = a |> f -- after f\n  |> g\n"),
        "let p = a\n  |> f -- after f\n  |> g\n"
    );
    assert_eq!(
        fmt("let b = a + b -- after b\n  + c\n"),
        "let b = a\n  + b -- after b\n  + c\n"
    );
    assert_eq!(
        fmt("let s = { (* c *) a: 1n }"),
        "let s = { (* c *) a: 1n }\n"
    );
    // Alone inside brackets: on its own line, and never in front of the
    // closer on the same line.
    assert_eq!(
        fmt("let u = ( -- inside unit\n)"),
        "let u = (\n  -- inside unit\n)\n"
    );
    assert_eq!(fmt("let e = { -- c\n}"), "let e = {\n  -- c\n}\n");
    assert_eq!(fmt("let e = [ -- c\n]"), "let e = [\n  -- c\n]\n");
    assert_eq!(
        fmt("let e = {}\nlet f = ()\nlet g = []"),
        "let e = {}\nlet f = ()\nlet g = []\n"
    );
    // A block comment on a line of its own stays on one.
    assert_eq!(fmt("(* block *) let y = 2n"), "(* block *)\nlet y = 2n\n");
}

#[test]
fn comments_in_blocks_keep_their_place() {
    assert_eq!(
        fmt("let c = if a then b -- after b\nelse c\nend"),
        "let c =\n  if a then\n    b -- after b\n  else c\n  end\n"
    );
    assert_eq!(
        fmt(
            "let c2 = if a then b\n-- before else if\nelse if d then e\n-- before else\nelse f\n-- before end\nend"
        ),
        "let c2 =\n  if a then b\n  -- before else if\n  else if d then e\n  -- before else\n  else f\n  -- before end\n  end\n"
    );
    assert_eq!(
        fmt(
            "let m = match v with\n-- first\n| 1n => 2n -- arm\n-- second\n| _ => 3n\n-- last\nend"
        ),
        "let m = match v with\n-- first\n| 1n => 2n -- arm\n-- second\n| _ => 3n\n-- last\nend\n"
    );
    assert_eq!(
        fmt("module M =\n  -- only a comment\nend"),
        "module M =\n  -- only a comment\nend\n"
    );
    assert_eq!(
        fmt(
            "let z = do\n  -- before a\n  let a = 1n -- after a\n  -- before return\n  return a\n  -- dangling in do\nend"
        ),
        "let z = do\n  -- before a\n  let a = 1n -- after a\n  -- before return\n  return a\n  -- dangling in do\nend\n"
    );
    assert_eq!(
        fmt("let h = handle g () with\n-- arm\n| !Log s => s\nend"),
        "let h = handle g () with\n-- arm\n| !Log s => s\nend\n"
    );
    assert_eq!(fmt("-- only\n-- comments\n"), "-- only comments\n");
    assert_eq!(fmt("let a = 1n\n-- tail"), "let a = 1n\n-- tail\n");
}

#[test]
fn comment_text_is_reflowed_by_paragraph() {
    let long = "-- A paragraph that is rather long and should be reflowed because it goes on and on past the width limit of one hundred columns.\n-- continues here\n--\n--   indented code sample\n-- - a list item\n-- 1. numbered\n-- ---\nlet x = 1n\n";
    assert_eq!(
        fmt(long),
        "-- A paragraph that is rather long and should be reflowed because it goes on and on past the width\n-- limit of one hundred columns. continues here\n--\n--   indented code sample\n-- - a list item\n-- 1. numbered\n-- ---\nlet x = 1n\n"
    );
    // A blank line between two comments keeps them two paragraphs, and
    // stays; a run at one indentation is refilled as one.
    assert_eq!(
        fmt("-- one\n-- two\n\n-- three\nlet after = 1n"),
        "-- one two\n\n-- three\nlet after = 1n\n"
    );
    assert_eq!(
        fmt("--no space\n--\t\tlet x = 1n"),
        "-- no space\n--   let x = 1n\n"
    );
    let word = "x".repeat(120);
    assert_eq!(
        fmt(&format!("-- {word} tail\nlet a = 1n")),
        format!("-- {word}\n-- tail\nlet a = 1n\n")
    );
    // A trailing comment that runs past the width wraps under its `--`.
    let trailing = "let x = 1n -- trailing comment that is quite long and would push the line past the width limit if it were not wrapped somewhere\n";
    assert_eq!(
        fmt(trailing),
        "let x = 1n -- trailing comment that is quite long and would push the line past the width limit if it\n           -- were not wrapped somewhere\n"
    );
    // A multi-line block comment is re-indented under its opener, and a
    // one-line one is left alone.
    assert_eq!(
        fmt("(*\n   multi-line block comment\n   with two lines\n*)\nlet y = 2n"),
        "(*\n  multi-line block comment with two lines\n*)\nlet y = 2n\n"
    );
    assert_eq!(
        fmt("(* first\n     second\n   third *)\nlet y = 2n"),
        "(* first\n    second\n  third\n*)\nlet y = 2n\n"
    );
    assert_eq!(
        fmt("(*   untouched   *)\nlet y = 2n"),
        "(*   untouched   *)\nlet y = 2n\n"
    );
}

#[test]
fn literals_are_copied_or_normalized() {
    assert_eq!(fmt("let raw = \\\\one"), "let raw = \\\\one\n");
    assert_eq!(
        fmt("let raw2 = \\\\one\n           \\\\two"),
        "let raw2 = \\\\one\n\\\\two\n"
    );
    assert_eq!(fmt("let rawarg = f \\\\raw"), "let rawarg = f \\\\raw\n");
    assert_eq!(
        fmt("let q = { \"quoted\": 1n, 007: 2n, \"b\": 3n }"),
        "let q = { \"quoted\": 1n, 7: 2n, \"b\": 3n }\n"
    );
    assert_eq!(
        fmt("let qt = #\"quoted tag\" 1n"),
        "let qt = #\"quoted tag\" 1n\n"
    );
    assert_eq!(
        fmt("let proj = p.007\nlet proj2 = p.\"x\""),
        "let proj = p.7\nlet proj2 = p.\"x\"\n"
    );
    assert_eq!(
        fmt(
            "let n007 = 007n\nlet n0i = -0i\nlet fx = 4n8\nlet r = 1.50\nlet r2 = 007.0\nlet r3 = -0.0\nlet r4 = 0.5\nlet r5 = 00\nlet b = true"
        ),
        "let n007 = 7n\nlet n0i = 0i\nlet fx = 4n8\nlet r = 1.5\nlet r2 = 7\nlet r3 = -0\nlet r4 = 0.5\nlet r5 = 0\nlet b = true\n"
    );
    assert_eq!(
        fmt("let big_real = 340282366920938463463374607431768211455"),
        "let big_real = 340282366920938463463374607431768211455\n"
    );
    assert_eq!(fmt("let s = \"a\\tb\\\"c\""), "let s = \"a\\tb\\\"c\"\n");
    assert_eq!(
        fmt("let unicode = \"ünïcödé\" "),
        "let unicode = \"ünïcödé\"\n"
    );
    assert_eq!(
        fmt(
            "let path = Math::Vec::zero\nlet op = !Log.write\nlet op2 = Sys::!Log\nlet op3 = Sys::!Log.write"
        ),
        "let path = Math::Vec::zero\nlet op = !Log.write\nlet op2 = Sys::!Log\nlet op3 = Sys::!Log.write\n"
    );
    assert_eq!(
        fmt(
            "let m = match v with | 1.50 => 1n | -1i => 2n | \"s\" => 3n | true => 4n | () => 5n | 4n8 => 6n end"
        ),
        "let m = match v with | 1.5 => 1n | -1i => 2n | \"s\" => 3n | true => 4n | () => 5n | 4n8 => 6n end\n"
    );
}

#[test]
fn operators_keep_their_grouping() {
    assert_eq!(
        fmt(
            "let neg = - 1\nlet nn = - -x\nlet n2 = -x\nlet n3 = -(a + b)\nlet nt = not a\nlet mut_ = mut x\nlet rd = ~cell\nlet lit = -1"
        ),
        "let neg = - 1\nlet nn = - -x\nlet n2 = -x\nlet n3 = -(a + b)\nlet nt = not a\nlet mut_ = mut x\nlet rd = ~cell\nlet lit = -1\n"
    );
    assert_eq!(
        fmt(
            "let last = f x #None\nlet mid = f #None x\nlet pay = #Some #None\nlet pay2 = #Some (f x)\nlet head = (#A 1n) 2n"
        ),
        "let last = f x #None\nlet mid = f (#None x)\nlet pay = #Some (#None)\nlet pay2 = #Some (f x)\nlet head = #A 1n 2n\n"
    );
    assert_eq!(
        fmt(
            "let bin = a + b - c\nlet bin2 = a * b / c + d\nlet bin3 = a and b or c xor d\nlet asg = cell := other := 1n\nlet asg2 = (a := b) := c"
        ),
        "let bin = a + b - c\nlet bin2 = a * b / c + d\nlet bin3 = a and b or c xor d\nlet asg = cell := other := 1n\nlet asg2 = (a := b) := c\n"
    );
    assert_eq!(
        fmt(
            "let paren = (a + b) * c\nlet paren2 = a + (b * c)\nlet paren3 = (a |> f) |> g\nlet paren4 = a |> (f |> g)\nlet paren5 = (fn x => x) y\nlet paren6 = f (fn x => x)"
        ),
        "let paren = (a + b) * c\nlet paren2 = a + b * c\nlet paren3 = a |> f |> g\nlet paren4 = a |> (f |> g)\nlet paren5 = (fn x => x) y\nlet paren6 = f (fn x => x)\n"
    );
    assert_eq!(
        fmt(
            "let paren7 = (match v with | 1n => f | _ => g end) x\nlet paren8 = f (match v with | 1n => 1n | _ => 2n end)\nlet paren9 = (f x).field\nlet paren10 = (a.0).1\nlet paren11 = f (a := b)\nlet paren12 = f (raise x)\nlet paren13 = a + (b + c)\nlet paren14 = (a + b) + c"
        ),
        "let paren7 = match v with | 1n => f | _ => g end x\nlet paren8 = f (match v with | 1n => 1n | _ => 2n end)\nlet paren9 = (f x).field\nlet paren10 = (a.0).1\nlet paren11 = f (a := b)\nlet paren12 = f (raise x)\nlet paren13 = a + (b + c)\nlet paren14 = a + b + c\n"
    );
    assert_eq!(
        fmt("let p = (a + b).x\nlet q = (#A).x\nlet r = (if a then b else c end).x"),
        "let p = (a + b).x\nlet q = (#A).x\nlet r = (if a then b else c end).x\n"
    );
}

#[test]
fn chains_break_before_their_operators() {
    assert_eq!(
        fmt("let pipe2 = a\n  |> f\n  |> g\nlet bin4 = a\n  + b\n  + c\nlet asg = cell :=\n  1n"),
        "let pipe2 = a\n  |> f\n  |> g\nlet bin4 = a\n  + b\n  + c\nlet asg = cell\n  := 1n\n"
    );
    assert_eq!(
        fmt(
            "let long_pipe = value_one |> some_function_one |> some_function_two |> some_function_three |> function_four"
        ),
        "let long_pipe =\n  value_one |> some_function_one |> some_function_two |> some_function_three |> function_four\n"
    );
    assert_eq!(
        fmt(
            "let long_pipe = value_one |> some_function_one |> some_function_two |> some_function_three |> some_function_four |> five"
        ),
        "let long_pipe = value_one\n  |> some_function_one\n  |> some_function_two\n  |> some_function_three\n  |> some_function_four\n  |> five\n"
    );
    assert_eq!(
        fmt(
            "let long_bin = first_operand_name + second_operand_name + third_operand_name + fourth_operand_name + fifth_operand_name"
        ),
        "let long_bin = first_operand_name\n  + second_operand_name\n  + third_operand_name\n  + fourth_operand_name\n  + fifth_operand_name\n"
    );
}

#[test]
fn conditionals_follow_the_corpus_style() {
    assert_eq!(
        fmt("let flat = if a then b else c end"),
        "let flat = if a then b else c end\n"
    );
    assert_eq!(
        fmt("let chain = if a then b else if c then d else e end"),
        "let chain = if a then b else if c then d else e end\n"
    );
    assert_eq!(
        fmt(
            "let width = if some_quite_long_predicate_expression_here then some_consequent_expression else if another_long_predicate then another_consequent else the_final_alternative_value end"
        ),
        "let width =\n  if some_quite_long_predicate_expression_here then some_consequent_expression\n  else if another_long_predicate then another_consequent\n  else the_final_alternative_value\n  end\n"
    );
    assert_eq!(
        fmt(
            "let long_branch = if a then some_extremely_long_consequent_expression_that_does_not_fit_on_the_line_with_the_predicate_at_all else b end"
        ),
        "let long_branch =\n  if a then\n    some_extremely_long_consequent_expression_that_does_not_fit_on_the_line_with_the_predicate_at_all\n  else b\n  end\n"
    );
    // Any newline between `then` and `end` breaks the chain.
    assert_eq!(
        fmt("let inlet = if a then\n  b\nelse c\nend"),
        "let inlet =\n  if a then b\n  else c\n  end\n"
    );
    assert_eq!(
        fmt(
            "let f = fn x => if x then 1n else 2n end\nlet f2 = fn x =>\n  if x then 1n else 2n end"
        ),
        "let f = fn x => if x then 1n else 2n end\nlet f2 = fn x =>\n  if x then 1n else 2n end\n"
    );
    assert_eq!(
        fmt(
            "let compare : Nat -> Nat -> Ordering = fn left right =>\n  if less_than left right then #Less\n  else if greater_than left right then #Greater\n  else #Equal\n  end"
        ),
        "let compare: Nat -> Nat -> Ordering = fn left right =>\n  if less_than left right then #Less\n  else if greater_than left right then #Greater\n  else #Equal\n  end\n"
    );
    // A parenthesized nested conditional is the same chain.
    assert_eq!(
        fmt("let n = if a then b else (if c then d else e end) end"),
        "let n = if a then b else if c then d else e end\n"
    );
}

#[test]
fn applications_expand_or_hug_their_last_argument() {
    assert_eq!(
        fmt(
            "let app = some_function_name first_argument second_argument third_argument fourth_argument fifth_argument sixth"
        ),
        "let app = some_function_name\n  first_argument\n  second_argument\n  third_argument\n  fourth_argument\n  fifth_argument\n  sixth\n"
    );
    assert_eq!(
        fmt(
            "let hug = some_function some_argument { first_field: some_value_expression, second_field: another_value_here }"
        ),
        "let hug = some_function some_argument {\n  first_field: some_value_expression,\n  second_field: another_value_here,\n}\n"
    );
    assert_eq!(
        fmt(
            "let hug2 = some_function_with_a_much_longer_name some_argument another_argument yet_another_argument { first_field: some_value_expression, second_field: another_value }"
        ),
        "let hug2 =\n  some_function_with_a_much_longer_name some_argument another_argument yet_another_argument {\n    first_field: some_value_expression,\n    second_field: another_value,\n  }\n"
    );
    assert_eq!(
        fmt("let hug3 = f [1n, 2n]\nlet hug4 = f (1n, 2n) x\nlet sig = f\n  a\n  b"),
        "let hug3 = f [1n, 2n]\nlet hug4 = f (1n, 2n) x\nlet sig = f\n  a\n  b\n"
    );
    assert_eq!(
        fmt(
            "let s = f {\n  a: 1n }\nlet fnlong = fn argument_one argument_two => some_function argument_one argument_two another_argument_here yes"
        ),
        "let s = f {\n  a: 1n,\n}\nlet fnlong = fn argument_one argument_two =>\n  some_function argument_one argument_two another_argument_here yes\n"
    );
    // The hugged form is refused when the head and the other arguments do
    // not fit in front of the brace even on a line of their own.
    assert_eq!(
        fmt(
            "let hug5 = some_function_with_a_much_longer_name some_argument another_argument yet_another_argument and_one_more { first_field: some_value_expression }"
        ),
        "let hug5 =\n  some_function_with_a_much_longer_name\n    some_argument\n    another_argument\n    yet_another_argument\n    and_one_more\n    { first_field: some_value_expression }\n"
    );
}

#[test]
fn delimited_literals_break_one_entry_per_line() {
    assert_eq!(
        fmt(
            "let one = (1n,)\nlet two = (1n, 2n,)\nlet arr = [1n, ..rest, 2n,]\nlet arr2 = []\nlet sp = { a: 1n, ..spread }\nlet st = {\n  a: 1n }\nlet empty = {}"
        ),
        "let one = (1n,)\nlet two = (1n, 2n)\nlet arr = [1n, ..rest, 2n]\nlet arr2 = []\nlet sp = { a: 1n, ..spread }\nlet st = {\n  a: 1n,\n}\nlet empty = {}\n"
    );
    assert_eq!(
        fmt(
            "let big = [first_element_name, second_element_name, third_element_name, fourth_element_name, fifth, sixth]"
        ),
        "let big = [\n  first_element_name,\n  second_element_name,\n  third_element_name,\n  fourth_element_name,\n  fifth,\n  sixth,\n]\n"
    );
    assert_eq!(
        fmt(
            "let bigt = (first_element_name, second_element_name, third_element_name, fourth_element_name, fifth, six)"
        ),
        "let bigt = (\n  first_element_name,\n  second_element_name,\n  third_element_name,\n  fourth_element_name,\n  fifth,\n  six,\n)\n"
    );
    assert_eq!(
        fmt(
            "let one = (some_very_long_element_name_that_goes_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on,)"
        ),
        "let one = (\n  some_very_long_element_name_that_goes_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on,\n)\n"
    );
    assert_eq!(
        fmt(
            "let long_struct = { first_field: some_value_expression, second_field: another_value_expression, third: third }"
        ),
        "let long_struct = {\n  first_field: some_value_expression,\n  second_field: another_value_expression,\n  third: third,\n}\n"
    );
    assert_eq!(
        fmt(
            "let spread_long = { first_field: some_value_expression, second_field: another_value_expression, ..the_rest }"
        ),
        "let spread_long = {\n  first_field: some_value_expression,\n  second_field: another_value_expression,\n  ..the_rest\n}\n"
    );
    assert_eq!(
        fmt("let nested = { outer: { inner: [1n, 2n] } }"),
        "let nested = { outer: { inner: [1n, 2n] } }\n"
    );
    assert_eq!(fmt("let m = match v with | { x, y: 1n, .. } => x | (a, b) => a | [first, ..rest, last] => first | [..] => 0n | #Some (#Ok x) => x | {} => 1n | ( a, ) => a | _ => 6n end"),
        "let m = fn v => match v with\n| { x, y: 1n, .. } => x\n| (a, b) => a\n| [first, ..rest, last] => first\n| [..] => 0n\n| #Some (#Ok x) => x\n| {} => 1n\n| (a,) => a\n| _ => 6n\nend\n".replace("let m = fn v => match v with\n", "let m = match v with\n"));
    assert_eq!(
        fmt(
            "let pat = fn v => match v with\n| { first_field, second_field: 1n, third_field: 2n, fourth_field: 3n, fifth_field: 4n, sixth: 5n } => 0n\n| [first_element, second_element, third_element, fourth_element, fifth_element, sixth_elements] => 1n\n| (first_element, second_element, third_element, fourth_element, fifth_element, sixth_elements) => 2n\nend"
        ),
        "let pat = fn v => match v with\n| {\n  first_field,\n  second_field: 1n,\n  third_field: 2n,\n  fourth_field: 3n,\n  fifth_field: 4n,\n  sixth: 5n,\n} => 0n\n| [\n  first_element,\n  second_element,\n  third_element,\n  fourth_element,\n  fifth_element,\n  sixth_elements,\n] => 1n\n| (\n  first_element,\n  second_element,\n  third_element,\n  fourth_element,\n  fifth_element,\n  sixth_elements,\n) => 2n\nend\n"
    );
}

#[test]
fn functions_matches_and_blocks_hug_their_arrows() {
    assert_eq!(
        fmt(
            "let mf = fn | #A => 1n | #B => 2n\nlet mf2 = fn\n  | #A => 1n\n  | #B => 2n\nlet mf3 = fn | #A => fn | #C => 3n | #D => 4n\nlet mf4 = fn | #A => (fn | #C => 3n) | #D => 4n"
        ),
        "let mf = fn | #A => 1n | #B => 2n\nlet mf2 = fn\n| #A => 1n\n| #B => 2n\nlet mf3 = fn | #A => fn | #C => 3n | #D => 4n\nlet mf4 = fn | #A => (fn | #C => 3n) | #D => 4n\n"
    );
    assert_eq!(
        fmt("let mf5 = fn\n  | #A => 1n\n  | #B => fn | #C => 3n\n  | #D => 4n"),
        "let mf5 = fn\n| #A => 1n\n| #B =>\n  fn\n  | #C => 3n\n  | #D => 4n\n"
    );
    assert_eq!(
        fmt(
            "let hd = handle g () with | !Log s => resume () | return x => x end\nlet hd2 = handle g () with\n  | !Log s => resume ()\n  | Sys::!Log.write s => s\n  | return x => x\nend\nlet hd3 = handle g () with end\nlet em = match x with end\nlet ed = do end\nlet rs = do return 1n end\nlet rs2 = do\n  return 1n\nend\nlet rs3 = do\n  return\n    1n\nend"
        ),
        "let hd = handle g () with | !Log s => resume () | return x => x end\nlet hd2 = handle g () with\n| !Log s => resume ()\n| Sys::!Log.write s => s\n| return x => x\nend\nlet hd3 = handle g () with end\nlet em = match x with end\nlet ed = do end\nlet rs = do return 1n end\nlet rs2 = do\n  return 1n\nend\nlet rs3 = do\n  return\n    1n\nend\n"
    );
    assert_eq!(
        fmt(
            "let nested = fn v => match v with\n| #A x => match x with\n  | 1n => 2n\n  | _ => 3n\n  end\n| #B => 0n\nend"
        ),
        "let nested = fn v => match v with\n| #A x =>\n  match x with\n  | 1n => 2n\n  | _ => 3n\n  end\n| #B => 0n\nend\n"
    );
    assert_eq!(
        fmt(
            "let arm_do = fn v => match v with\n| #A => do\n  let y = 1n\n  return y\nend\n| #B => 0n\nend"
        ),
        "let arm_do = fn v => match v with\n| #A => do\n  let y = 1n\n  return y\nend\n| #B => 0n\nend\n"
    );
    assert_eq!(
        fmt(
            "let arm_long = fn v => match v with\n| #A => some_extremely_long_function_name_here first_argument second_argument third_argument fourth\n| #B => 0n\nend"
        ),
        "let arm_long = fn v => match v with\n| #A => some_extremely_long_function_name_here first_argument second_argument third_argument fourth\n| #B => 0n\nend\n"
    );
    assert_eq!(
        fmt(
            "let arm_long2 = fn v => match v with\n| #A => some_extremely_long_function_name_here first_argument second_argument third_argument fourth_argument fifth_argument\n| #B => 0n\nend"
        ),
        "let arm_long2 = fn v => match v with\n| #A =>\n  some_extremely_long_function_name_here\n    first_argument\n    second_argument\n    third_argument\n    fourth_argument\n    fifth_argument\n| #B => 0n\nend\n"
    );
    assert_eq!(
        fmt(
            "let raise_it = fn _ => raise 0n\nlet ret_match = do\n  return match x with\n  | 1n => 2n\n  | _ => 3n\n  end\nend\nlet fnfn = fn a => fn b => fn c => a"
        ),
        "let raise_it = fn _ => raise 0n\nlet ret_match = do\n  return match x with\n  | 1n => 2n\n  | _ => 3n\n  end\nend\nlet fnfn = fn a => fn b => fn c => a\n"
    );
    assert_eq!(
        fmt(
            "let let_if = if p then some_quite_long_consequent_expression_here else another_quite_long_alternative_here end"
        ),
        "let let_if =\n  if p then some_quite_long_consequent_expression_here else another_quite_long_alternative_here end\n"
    );
    // A definition too long even with its match hugging the `=` moves the
    // function down a line; one that fits keeps it up.
    assert_eq!(
        fmt(
            "let collect_entries: FsEntries -> [fs::DirEntry] -> [fs::DirEntry] = fn entries values => match entries with\n  | #Cons (entry, rest) => collect_entries rest (array_push values entry)\n  | #None => values\nend"
        ),
        "let collect_entries: FsEntries -> [fs::DirEntry] -> [fs::DirEntry] = fn entries values =>\n  match entries with\n  | #Cons (entry, rest) => collect_entries rest (array_push values entry)\n  | #None => values\n  end\n"
    );
    assert_eq!(
        fmt(
            "let write_bytes = fn path bytes =>\n  !FileSystem.write_bytes { path: path, bytes: bytes }\nlet x =\n  1n\nlet y = do\n  let a = 1n\n\n  return a\nend"
        ),
        "let write_bytes = fn path bytes =>\n  !FileSystem.write_bytes { path: path, bytes: bytes }\nlet x =\n  1n\nlet y = do\n  let a = 1n\n\n  return a\nend\n"
    );
}

#[test]
fn definitions_break_their_headers_last() {
    assert_eq!(
        fmt(
            "let swap_fields :\n  { a when 'a: Nat, b when 'b: Nat }\n  -> { a when 'b: Nat, b when 'a: Nat }\n  where 'a != 'b\n  = fn v =>\n    match v with\n      | {a} => { b: a }\n      | {b} => { a: b }\n    end"
        ),
        "let swap_fields:\n  { a when 'a: Nat, b when 'b: Nat }\n  -> { a when 'b: Nat, b when 'a: Nat }\n  where 'a != 'b =\n  fn v =>\n    match v with\n    | { a } => { b: a }\n    | { b } => { a: b }\n    end\n"
    );
    assert_eq!(
        fmt(
            "let very_long_definition_name: SomeLongTypeName -> AnotherLongTypeName -> YetAnotherLongTypeName -> Result = fn a b c => a"
        ),
        "let very_long_definition_name:\n  SomeLongTypeName -> AnotherLongTypeName -> YetAnotherLongTypeName -> Result = fn a b c => a\n"
    );
    assert_eq!(
        fmt(
            "let very_long_definition_name: SomeLongTypeName -> AnotherLongTypeName -> YetAnotherLongTypeName -> SomeResultTypeName -> MoreType = fn a b c => a"
        ),
        "let very_long_definition_name:\n  SomeLongTypeName\n  -> AnotherLongTypeName\n  -> YetAnotherLongTypeName\n  -> SomeResultTypeName\n  -> MoreType = fn a b c => a\n"
    );
    assert_eq!(
        fmt("let both : { x when 'a: 'c, y when 'b: 'd } -> {} where 'a = 'b\n  = fn v => v"),
        "let both: { x when 'a: 'c, y when 'b: 'd } -> {} where 'a = 'b =\n  fn v => v\n"
    );
    assert_eq!(
        fmt("let { a, b } = pair\nlet (x, y) = pair"),
        "let { a, b } = pair\nlet (x, y) = pair\n"
    );
}

#[test]
fn types_print_by_their_grammar() {
    assert_eq!(
        fmt(
            "type Pair 'a 'b = { first: 'a, second: 'b }\ntype Sum = #A Nat | #B (when 'p) | \\#C | ..'r\ntype Empty = |\ntype TailOnly = | ..'r\ntype Row = { a when 'p: Nat, \\b, ..'r }\ntype Anon = { a when _: Nat, .. }"
        ),
        "type Pair 'a 'b = { first: 'a, second: 'b }\ntype Sum = #A Nat | #B (when 'p) | \\#C | ..'r\ntype Empty = |\ntype TailOnly = | ..'r\ntype Row = { a when 'p: Nat, \\b, ..'r }\ntype Anon = { a when _: Nat, .. }\n"
    );
    assert_eq!(
        fmt(
            "type Fun = (Nat -> Nat + !Log + !Ask Nat (when 'p) + \\!IO + ..'e) -> Nat + |\ntype Eff = Nat -> Nat + ..'e\ntype Nested = Nat -> (Nat -> Nat) + !Log\ntype Chain = Nat -> Nat -> Nat + !Log\ntype Left = (Nat -> Nat) -> Nat\ntype SumIn = (#A | #B) -> Nat\ntype RowIn = (!A + !B) -> Nat"
        ),
        "type Fun = (Nat -> Nat + !Log + !Ask Nat (when 'p) + \\!IO + ..'e) -> Nat + |\ntype Eff = Nat -> Nat + ..'e\ntype Nested = Nat -> (Nat -> Nat) + !Log\ntype Chain = Nat -> Nat -> Nat + !Log\ntype Left = (Nat -> Nat) -> Nat\ntype SumIn = (#A | #B) -> Nat\ntype RowIn = (!A + !B) -> Nat\n"
    );
    assert_eq!(
        fmt(
            "type M = mut 'r Nat\ntype M2 = mut (Region 'a) (Pair Nat Nat)\ntype T = (Nat, Nat)\ntype One = (Nat,)\ntype A = [Nat]\ntype H = _\ntype U = ()\ntype Q = Math::Pair Nat Nat\ntype Ap = Pair (Pair Nat Nat) [Nat]\ntype E = Runner (!Log + !IO)\ntype R = Runner (..'r)"
        ),
        "type M = mut 'r Nat\ntype M2 = mut (Region 'a) (Pair Nat Nat)\ntype T = (Nat, Nat)\ntype One = (Nat,)\ntype A = [Nat]\ntype H = _\ntype U = ()\ntype Q = Math::Pair Nat Nat\ntype Ap = Pair (Pair Nat Nat) [Nat]\ntype E = Runner (!Log + !IO)\ntype R = Runner (..'r)\n"
    );
    assert_eq!(
        fmt(
            "type Long = { first_field_name: SomeLongTypeName, second_field_name: AnotherLongTypeName, third: Third }"
        ),
        "type Long = {\n  first_field_name: SomeLongTypeName,\n  second_field_name: AnotherLongTypeName,\n  third: Third,\n}\n"
    );
    assert_eq!(
        fmt(
            "type LongSum = #FirstCase SomeLongTypeName | #SecondCase AnotherLongTypeName | #ThirdCase Third | #Fourth"
        ),
        "type LongSum =\n  #FirstCase SomeLongTypeName | #SecondCase AnotherLongTypeName | #ThirdCase Third | #Fourth\n"
    );
    assert_eq!(
        fmt(
            "type LongSum = #FirstCase SomeLongTypeName | #SecondCase AnotherLongTypeName | #ThirdCase Third | #FourthCase Fourth | #Fifth"
        ),
        "type LongSum =\n  | #FirstCase SomeLongTypeName\n  | #SecondCase AnotherLongTypeName\n  | #ThirdCase Third\n  | #FourthCase Fourth\n  | #Fifth\n"
    );
    assert_eq!(
        fmt("type ErrorKind =\n  | #NotFound\n  | #Other\ntype Tail =\n  | #A\n  | ..'r"),
        "type ErrorKind =\n  | #NotFound\n  | #Other\ntype Tail =\n  | #A\n  | ..'r\n"
    );
    assert_eq!(
        fmt(
            "type W = { x when 'a: Nat, y when 'b: Nat } where 'a = 'b; not 'a and 'b or 'a != 'b; ('a or 'b) and 'a; not ('a and 'b); 'a = ('b or 'a)"
        ),
        "type W =\n  { x when 'a: Nat, y when 'b: Nat }\n  where 'a = 'b; not 'a and 'b or 'a != 'b; ('a or 'b) and 'a; not ('a and 'b); 'a = 'b or 'a\n"
    );
    assert_eq!(
        fmt(
            "type LongTuple = (SomeLongTypeName, AnotherLongTypeName, YetAnotherLongTypeName, OneMoreLongTypeName)"
        ),
        "type LongTuple = (\n  SomeLongTypeName,\n  AnotherLongTypeName,\n  YetAnotherLongTypeName,\n  OneMoreLongTypeName,\n)\n"
    );
    assert_eq!(
        fmt(
            "type Fields = {\n  a: Nat }\ntype Bar = { a: Nat -> Nat + !Log, b: (Nat, Nat) -> [Nat] }"
        ),
        "type Fields = {\n  a: Nat,\n}\ntype Bar = { a: Nat -> Nat + !Log, b: (Nat, Nat) -> [Nat] }\n"
    );
}

#[test]
fn declarations_print_by_their_grammar() {
    assert_eq!(
        fmt(
            "effect Empty\neffect Alias = !Log + !Ask Nat + ..'e\neffect Unnamed 'a = () -> 'a\neffect Named = { get: () -> Nat, set: Nat -> () }\neffect Named2 = {\n  get: () -> Nat }"
        ),
        "effect Empty\neffect Alias = !Log + !Ask Nat + ..'e\neffect Unnamed 'a = () -> 'a\neffect Named = { get: () -> Nat, set: Nat -> () }\neffect Named2 = {\n  get: () -> Nat,\n}\n"
    );
    assert_eq!(
        fmt("effect Alias 'a =\n  !Log + !Ask 'a\neffect Unnamed 'a =\n  () -> 'a"),
        "effect Alias 'a =\n  !Log + !Ask 'a\neffect Unnamed 'a =\n  () -> 'a\n"
    );
    assert_eq!(
        fmt(
            "extern add : fn(Nat, Nat) -> Nat + !Log = \"add\"\nextern annotated : @async fn(Nat) -> Nat = \"a\"\nextern plain : Nat -> Nat where 'a = 'a = \"p\"\nextern grouped : (fn(Nat) -> Nat) = \"g\"\nextern none : fn() -> Nat = \"n\"\nextern nested : fn(fn(Nat) -> Nat, Nat) -> Nat = \"n\""
        ),
        "extern add: fn(Nat, Nat) -> Nat + !Log = \"add\"\nextern annotated: @async fn(Nat) -> Nat = \"a\"\nextern plain: Nat -> Nat where 'a = 'a = \"p\"\nextern grouped: (fn(Nat) -> Nat) = \"g\"\nextern none: fn() -> Nat = \"n\"\nextern nested: fn(fn(Nat) -> Nat, Nat) -> Nat = \"n\"\n"
    );
    assert_eq!(
        fmt(
            "extern clamp : fn(Nat, Nat, Nat) -> Nat = \"(value, minimum, maximum) => Math.min(Math.max(value, minimum), maximum)\""
        ),
        "extern clamp: fn(Nat, Nat, Nat) -> Nat =\n  \"(value, minimum, maximum) => Math.min(Math.max(value, minimum), maximum)\"\n"
    );
    assert_eq!(
        fmt(
            "extern long_parameters : fn(SomeLongTypeName, AnotherLongTypeName, YetAnotherLongTypeName, OneMore) -> Nat = \"f\""
        ),
        "extern long_parameters: fn(\n  SomeLongTypeName,\n  AnotherLongTypeName,\n  YetAnotherLongTypeName,\n  OneMore,\n) -> Nat = \"f\"\n"
    );
    assert_eq!(
        fmt(
            "module Inline = let a = 1n end\nmodule Broken =\n  let a = 1n\n\n  let b = 2n\nend\nmodule Elsewhere\nmodule Empty = end\nmodule Nested = module Inner = let x = 1n end end"
        ),
        "module Inline = let a = 1n end\nmodule Broken =\n  let a = 1n\n\n  let b = 2n\nend\nmodule Elsewhere\nmodule Empty = end\nmodule Nested = module Inner = let x = 1n end end\n"
    );
    assert_eq!(
        fmt(
            "module Long = let first_definition = 1n let second_definition = 2n let third_definition = 3n let fourth = 4n end"
        ),
        "module Long =\n  let first_definition = 1n\n  let second_definition = 2n\n  let third_definition = 3n\n  let fourth = 4n\nend\n"
    );
}

#[test]
fn attributes_go_one_per_line_with_their_data() {
    let source = "@doc \"Adds\"\n@since 2n\n@shape (1n, \"two\")\n@one (1n,)\n@tags [\"core\", \"demo\"]\n@if { target: \"js\", platform: \"web\" }\n@stability #Experimental\n@retired #Removed 3n\n@deep #Outer #Inner\n@unit ()\n@none []\n@empty {}\n@real 1.50\n@neg -3i\n@fixed 4n8\n@bool true\nlet described = fn a => a\n";
    assert_eq!(
        fmt(source),
        source
            .replace("@real 1.50", "@real 1.5")
            .replace("@deep #Outer #Inner", "@deep #Outer (#Inner)")
    );
    assert_eq!(fmt("@a @b let x = 1n"), "@a\n@b\nlet x = 1n\n");
    assert_eq!(
        fmt(
            "@tags [\"first_long_tag_name\", \"second_long_tag_name\", \"third_long_tag_name\", \"fourth_long_tag_names\"]\nlet x = 1n"
        ),
        "@tags [\n  \"first_long_tag_name\",\n  \"second_long_tag_name\",\n  \"third_long_tag_name\",\n  \"fourth_long_tag_names\",\n]\nlet x = 1n\n"
    );
    assert_eq!(
        fmt(
            "@if { target: \"some_long_target_name\", platform: \"some_long_platform_name\", extra: \"another_long_value\" }\nlet x = 1n"
        ),
        "@if {\n  target: \"some_long_target_name\",\n  platform: \"some_long_platform_name\",\n  extra: \"another_long_value\",\n}\nlet x = 1n\n"
    );
    assert_eq!(
        fmt(
            "@shape (\"first_long_element_name\", \"second_long_element_name\", \"third_long_element_name\", \"fourth_one\")\nlet x = 1n"
        ),
        "@shape (\n  \"first_long_element_name\",\n  \"second_long_element_name\",\n  \"third_long_element_name\",\n  \"fourth_one\",\n)\nlet x = 1n\n"
    );
}

#[test]
fn errors_are_copied_around_and_reported() {
    let source = "let ok = 1n\nlet broken = fn x => (\nlet after = 2n\nmodule M =\n  let inner_ok = 1n\n  let inner_bad = }\n  let inner_after = 3n\nend\nlet blk = do\n  let good = 1n\n  let bad = )\n  let good2 = 2n\n  return good\nend\nlet ret_bad = do\n  let good = 1n\n  return ) oops\nend\nlet after_return = do\n  return 1n\n  let stray = 2n\nend\n@stray\nlet attributed_bad = fn =>\nlet attr_in_block = do\n  @inner let q = 1n\n  return q\nend\nlet bare = do return end\nlet tail   =   1n\n-- final comment\n";
    let out = formatted(source);
    assert!(out.has_errors());
    assert_eq!(
        out.text,
        "let ok = 1n\nlet broken = fn x => (\nlet after = 2n\nmodule M =\n  let inner_ok = 1n\n  let inner_bad = }\n  let inner_after = 3n\nend\nlet blk = do\n  let good = 1n\n  let bad = )\n  let good2 = 2n\n  return good\nend\nlet ret_bad = do\n  let good = 1n\n  return ) oops\nend\nlet after_return = do\n  return 1n\n  let stray = 2n\nend\n@stray\nlet attributed_bad = fn =>\nlet attr_in_block = do\n  @inner\n  let q = 1n\n  return q\nend\nlet bare = do return end\nlet tail = 1n\n-- final comment\n"
    );
    assert_eq!(fmt_lossy(&out.text), out.text);
    // A lexical error makes its statement opaque too, and so does a
    // recovered expression inside a formatted one.
    assert_eq!(
        fmt_lossy("let   s = \"unterminated\nlet   t = 1n"),
        "let   s = \"unterminated\nlet t = 1n\n"
    );
    assert_eq!(
        fmt_lossy("let   s = $\nlet   t = 1n"),
        "let   s = $\nlet t = 1n\n"
    );
    assert_eq!(
        fmt_lossy("let   s = fn x => return x\nlet   t = 1n"),
        "let   s = fn x => return x\nlet t = 1n\n"
    );
    assert_eq!(
        fmt_lossy("let   a = 1n\n@trailing"),
        "let a = 1n\n@trailing\n"
    );
    assert_eq!(
        fmt_lossy(
            "let   a = do\n  let x = 1n\n  return match x with\n    | 1n => (\n  end\nend\nlet   b = 2n"
        ),
        "let   a = do\n  let x = 1n\n  return match x with\n    | 1n => (\n  end\nend\nlet b = 2n\n"
    );
    // An opaque statement keeps its inner indentation relative to its first
    // line, wherever it lands.
    assert_eq!(
        fmt_lossy("module M =\n    let bad = )\n      continued\n    let   good = 1n\nend"),
        "module M =\n  let bad = )\n    continued\n  let good = 1n\nend\n"
    );
    assert_eq!(
        fmt_lossy(
            "let a = 1n\r\nlet b = do\r\n  let x = 1n\r\n  type T = Nat\r\n  return x\r\nend\r\n"
        ),
        "let a = 1n\nlet b = do\n  let x = 1n\n  type T = Nat\n  return x\nend\n"
    );
    assert_eq!(
        fmt_lossy("-- c\r\n(* a\r\n b *)\r\nlet x = \\\\a\r\n  \\\\b\r\n"),
        "-- c\n(* a\n  b\n*)\nlet x = \\\\a\n\\\\b\n"
    );
    assert_eq!(fmt_lossy("let (x) = 1n"), "let x = 1n\n");
}

#[test]
fn width_is_measured_in_characters() {
    let name = "ä".repeat(90);
    assert_eq!(fmt(&format!("let x = {name}")), format!("let x = {name}\n"));
    let exact = format!("let x = {}", "a".repeat(92));
    assert_eq!(exact.len(), 100);
    assert_eq!(
        fmt(&format!("{exact} |> f")),
        format!("let x =\n  {} |> f\n", "a".repeat(92))
    );
    let long = "a".repeat(90);
    assert_eq!(
        fmt(&format!("let x = {long} |> f |> g")),
        format!("let x = {long}\n  |> f\n  |> g\n")
    );
    assert_eq!(fmt(&exact), format!("{exact}\n"));
}

/// A comment on the line before an operand goes in front of its operator,
/// one before a case in front of its bar, and one between a definition's
/// attributes and its keyword between them.
#[test]
fn comments_before_operators_stay_in_front_of_them() {
    assert_eq!(
        fmt("let p = a\n  -- about f\n  |> f\n  |> g"),
        "let p = a\n  -- about f\n  |> f\n  |> g\n"
    );
    assert_eq!(
        fmt("let b = a\n  -- about b\n  + b\nlet c = cell\n  -- about the value\n  := 1n"),
        "let b = a\n  -- about b\n  + b\nlet c = cell\n  -- about the value\n  := 1n\n"
    );
    assert_eq!(
        fmt("let f = g\n  -- about a\n  a\n  b"),
        "let f = g\n  -- about a\n  a\n  b\n"
    );
    assert_eq!(
        fmt("type T =\n  Nat\n  -- about the result\n  -> Nat"),
        "type T =\n  Nat\n  -- about the result\n  -> Nat\n"
    );
    assert_eq!(
        fmt("type T =\n  -- first\n  | #A\n  -- second\n  | #B\ntype E = | -- tail\n  ..'r"),
        "type T =\n  -- first\n  | #A\n  -- second\n  | #B\ntype E =\n  | -- tail\n  ..'r\n"
    );
    assert_eq!(
        fmt("@doc \"x\"\n-- about x\nlet x = 1n\n@a\n(* inline *) type T = Nat"),
        "@doc \"x\"\n-- about x\nlet x = 1n\n@a\n(* inline *)\ntype T = Nat\n"
    );
}

#[test]
fn indented_comment_lines_stay_as_written() {
    assert_eq!(
        fmt("-- para\n--   deeper one\n--   deeper two\n-- para again\nlet x = 1n"),
        "-- para\n--   deeper one\n--   deeper two\n-- para again\nlet x = 1n\n"
    );
    assert_eq!(
        fmt("(*\n  first\n    indented\n  second\n*)\nlet x = 1n"),
        "(*\n  first\n    indented\n  second\n*)\nlet x = 1n\n"
    );
    // A block comment beside code that spans lines is re-indented too.
    assert_eq!(
        fmt("let x = 1n (* a\n  b *)\nlet y = (* c\nd *) 2n"),
        "let x = 1n (* a\n  b\n*)\nlet y = (* c\n  d\n*) 2n\n"
    );
}

/// The break the author made is between `then` and `end`; one in front of
/// `then` is not one.
#[test]
fn a_newline_before_then_does_not_break_a_conditional() {
    assert_eq!(
        fmt("let c = if a\n  then b else c end"),
        "let c = if a then b else c end\n"
    );
}

#[test]
fn raw_strings_keep_their_trailing_spaces_and_end_their_lines() {
    assert_eq!(
        fmt("let r = \\\\a \nlet s = \\\\b  \n           \\\\c "),
        "let r = \\\\a \nlet s = \\\\b  \n\\\\c \n"
    );
    assert_eq!(
        fmt("let a = [\n  \\\\a\n  , 1n\n]"),
        "let a = [\n  \\\\a\n  ,\n  1n,\n]\n"
    );
    assert_eq!(
        fmt("let t = (\\\\a\n, 1n)"),
        "let t = (\n  \\\\a\n  ,\n  1n,\n)\n"
    );
}

#[test]
fn effect_rows_print_their_marks_and_paths() {
    assert_eq!(
        fmt(
            "type T = Nat -> Nat + \\Sys::!Log + Sys::!Ask Nat + ..'e\ntype U = Nat -> Nat + \\!IO"
        ),
        "type T = Nat -> Nat + \\Sys::!Log + Sys::!Ask Nat + ..'e\ntype U = Nat -> Nat + \\!IO\n"
    );
}

#[test]
fn shorthand_arms_keep_the_parentheses_that_hand_back_a_bar() {
    assert_eq!(
        fmt(
            "let m = fn | #A => (fn x => fn | #C => 3n) | #D => 4n\nlet m2 = fn | #A => (raise fn | #C => 3n) | #D => 4n\nlet m3 = fn | #A => (fn x => x) | #D => 4n"
        ),
        "let m = fn | #A => (fn x => fn | #C => 3n) | #D => 4n\nlet m2 = fn | #A => (raise fn | #C => 3n) | #D => 4n\nlet m3 = fn | #A => fn x => x | #D => 4n\n"
    );
    assert_eq!(
        fmt(
            "let a = fn v => match v with\n| #A => fn x => do\n  let y = x\n  return y\nend\n| #B => 0n\nend"
        ),
        "let a = fn v => match v with\n| #A => fn x => do\n  let y = x\n  return y\nend\n| #B => 0n\nend\n"
    );
}

#[test]
fn comments_inside_copied_regions_and_empty_blocks() {
    // A comment inside a dropped region is part of the copy.
    assert_eq!(
        fmt_lossy("let bad = ) -- c\n oops\n\n  more\nlet   ok = 1n"),
        "let bad = ) -- c\n oops\n\n  more\nlet ok = 1n\n"
    );
    assert_eq!(
        fmt("let x = do\n  (* c *)\nend"),
        "let x = do\n  (* c *)\nend\n"
    );
    assert_eq!(
        fmt("(*\n  a\n\n  b\n*)\nlet x = 1n"),
        "(*\n  a\n\n  b\n*)\nlet x = 1n\n"
    );
    assert_eq!(
        fmt("let s = {\n  (* c *)\n  a: 1n }\nextern f : (fn(Nat) -> Nat\n  -- c\n) = \"x\""),
        "let s = {\n  (* c *)\n  a: 1n,\n}\nextern f: (fn(Nat) -> Nat\n  -- c\n) = \"x\"\n"
    );
    assert_eq!(
        fmt("let x = do\n  let a = 1n -- c\n  -- d\n  return a\nend"),
        "let x = do\n  let a = 1n -- c\n  -- d\n  return a\nend\n"
    );
}

/// A refused signature inside an `effect` makes the whole declaration
/// opaque, and a broken binding anywhere inside an expression makes only
/// its own block opaque.
#[test]
fn errors_inside_nested_blocks_stay_local() {
    assert_eq!(
        fmt_lossy("effect E = { get:   Nat }\nlet   a = 1n"),
        "effect E = { get:   Nat }\nlet a = 1n\n"
    );
    let source = "let   x = {\n  a: do let b = ) return 1n end,\n  ..do let c = ) return 2n end\n}\nlet   y = (do let b = ) return 1n end, [do let c = ) return 2n end])\nlet   z = #Some (do let d = ) return 3n end)\nlet   w = fn | #A => do let e = ) return 4n end\nlet   v = match do let f = ) return 5n end with | _ => do let g = ) return 6n end end\nlet   u = if do let h = ) return true end then 1n else 2n end\nlet   t = handle do let i = ) return 7n end with | !Log s => do let j = ) return 8n end end\nlet   s = raise do let k = ) return 9n end\nlet   r = do\n  let inner = do let l = ) return 10n end\n  return inner\nend\nlet   q = do\n  let m = ) oops\n  return 1n\nend\n";
    let text = fmt_lossy(source);
    for name in ["x", "y", "z", "w", "v", "u", "t", "s", "r", "q"] {
        assert!(text.contains(&format!("let {name} = ")), "{text}");
    }
    assert!(text.contains("let b = )"));
    assert!(text.contains("  let m = ) oops\n  return 1n\n"));
    // A one-line copied region is measured like any text.
    assert_eq!(
        fmt_lossy("let p = do let bad = ) return 1n end"),
        "let p = do let bad = ) return 1n end\n"
    );
    let long = "a".repeat(80);
    assert_eq!(
        fmt_lossy(&format!(
            "let p = do let bad = ) {long} let ok = 1n return ok end"
        )),
        format!("let p = do\n  let bad = ) {long}\n  let ok = 1n\n  return ok\nend\n")
    );
}

/// Hidden types and hidden patterns print as written: the type's body runs
/// to the end of the type and needs no parentheses, while a hidden type as an
/// input or an argument, and a hidden pattern's carried payload, keep theirs.
#[test]
fn hidden_types_and_patterns_format_as_written() {
    assert_eq!(
        fmt("let x : hide 'a => 'a -> 'a = f"),
        "let x: hide 'a => 'a -> 'a = f\n"
    );
    assert_eq!(
        fmt("type Any = hide 'a => { mirror: Mirror 'a, value: 'a }"),
        "type Any = hide 'a => { mirror: Mirror 'a, value: 'a }\n"
    );
    assert_eq!(
        fmt("let g : (hide 'a => 'a) -> Mirror (hide 'b => Box 'b) = f"),
        "let g: (hide 'a => 'a) -> Mirror (hide 'b => Box 'b) = f\n"
    );
    // A row written after the result belongs to the arrow. A hidden result
    // would take it inside its body, so it keeps its parentheses; with
    // nothing after it, the body running to the end is the same type and
    // the parentheses go.
    assert_eq!(
        fmt("let h : Nat -> (hide 'a => Nat -> 'a) + !Log = f"),
        "let h: Nat -> (hide 'a => Nat -> 'a) + !Log = f\n"
    );
    assert_eq!(
        fmt("let i : Nat -> (hide 'a => Nat -> 'a) = f"),
        "let i: Nat -> hide 'a => Nat -> 'a = f\n"
    );
    assert_eq!(
        fmt("let f = fn b => match b with | hide 'i { m, v } => m | hide 'x #Some y => y end"),
        "let f = fn b => match b with | hide 'i { m, v } => m | hide 'x (#Some y) => y end\n"
    );
    assert_eq!(
        fmt("let g = fn b => match b with | #Some hide 'z z => z end"),
        "let g = fn b => match b with | #Some (hide 'z z) => z end\n"
    );
    // Comments inside both forms stay where they were written.
    assert_eq!(
        fmt("type Any = hide 'a => {\n  -- the witness\n  mirror: Mirror 'a,\n  value: 'a,\n}\n"),
        "type Any = hide 'a => {\n  -- the witness\n  mirror: Mirror 'a,\n  value: 'a,\n}\n"
    );
}

#[test]
fn using_groups_and_comments_survive_formatting() {
    assert_eq!(fmt("using ::{dep}"), "using ::{dep}\n");
    assert_eq!(
        fmt("using ::dep::{self as external,value}"),
        "using ::dep::{self as external, value}\n"
    );
    assert_eq!(
        fmt("using {::dep as external,local}"),
        "using {::dep as external, local}\n"
    );
    assert_eq!(
        fmt("using A::{self as a,x,B::{*,y as z}}"),
        "using A::{self as a, x, B::{*, y as z}}\n"
    );
    let source = "using A::{\n  x, -- kept\n  (* nested (* note *) *) y as z\n}\nlet value = x\n";
    let output = fmt(source);
    assert!(output.contains("-- kept"));
    assert!(output.contains("(* nested (* note *) *)"));
    for source in [
        "using A::{::dep}",
        "using A *",
        "using A {x}",
        "using A::",
        "using A::{x",
        "using A as",
    ] {
        assert!(formatted(source).has_errors(), "{source}");
    }
}
