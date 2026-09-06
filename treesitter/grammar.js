/**
 * @file Tree-sitter grammar for ruddy (`.hc`)
 *
 * Mirrors `src/token.rs` and `src/parse.rs`. Where the two could disagree,
 * `src/parse.rs` is the source of truth: every rule below is named after the
 * production it stands for there.
 *
 * `extras` is whitespace and the two comment forms `token::lex` recognizes.
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// A name is a letter or an underscore followed by name characters, exactly as
// `token::word` reads one — except that a lone `_` is the wildcard and never an
// identifier, which is why the underscore branch demands a character after it.
const IDENT = /[\p{Alphabetic}][\p{Alphabetic}\p{N}_]*|_[\p{Alphabetic}\p{N}_]+/;

// The three sigilled labels: a tag, an effect, and a variable. Each is one
// token, and the name inside it starts the way an identifier does — `#_` and
// `#_x` are labels, and `#1x` is not.
const TAG = /#[\p{Alphabetic}_][\p{Alphabetic}\p{N}_]*/;
const EFFECT = /![\p{Alphabetic}_][\p{Alphabetic}\p{N}_]*/;
const VARIABLE = /'[\p{Alphabetic}_][\p{Alphabetic}\p{N}_]*/;
// The fourth: an attribute's key, `@deprecated`, which keeps the tag's rules —
// sigil and name are one token, and the name starts the way an identifier does.
const ATTRIBUTE = /@[\p{Alphabetic}_][\p{Alphabetic}\p{N}_]*/;

const PREC = {
  // `f x y` groups to the left, and stops in front of anything that begins no
  // argument.
  assignment: 1,
  pipeline: 2,
  booleanOr: 3,
  booleanXor: 4,
  booleanAnd: 5,
  addition: 6,
  multiplication: 7,
  unary: 8,
  application: 9,
  // A tag takes its payload before an application takes another argument, so
  // `f #A 1` is `f` applied to `#A 1`.
  tag: 7,
  // `f p.x` reaches into the record before passing it along.
  projection: 8,
};

/**
 * One or more `rule`, separated by `sep`.
 *
 * @param {RuleOrLiteral} sep
 * @param {RuleOrLiteral} rule
 * @returns {SeqRule}
 */
function sepBy1(sep, rule) {
  return seq(rule, repeat(seq(sep, rule)));
}

/** A structural field label, including compiler-invalid numeric forms. */
function fieldLabel($) {
  return choice(
    $.identifier,
    $.string,
    $.numeric_field,
    alias($._malformed_numeric_field, $.ERROR),
  );
}

module.exports = grammar({
  name: 'ruddy',

  word: $ => $.identifier,

  // The words `token::lex` reserves: no position reads one of these as a name.
  // `when` and `where` are deliberately absent — they are contextual, read by
  // spelling at the few positions that want them and ordinary identifiers
  // everywhere else.
  reserved: {
    global: _ => [
      'let',
      'extern',
      'do',
      'return',
      'if',
      'then',
      'else',
      'type',
      'end',
      'with',
      'match',
      'fn',
      'effect',
      'handle',
      'raise',
      'and',
      'or',
      'xor',
      'not',
      'mut',
      'module',
      'true',
      'false',
    ],
  },

  extras: $ => [/\s/, $.line_comment, $.block_comment],

  // This is a precedence-only choice, not a syntax node in the tree.
  inline: $ => [
    $.binary_expression,
    $._boolean_or,
    $._boolean_xor,
    $._boolean_and,
  ],

  conflicts: $ => [
    // `where 'a = 'b = v` compares two presences and then defines `v`;
    // `where 'a = v` is the clause `'a` and then the definition's own `=`.
    // Which one an `=` is cannot be known until the value after it is read,
    // so both readings are carried until one of them fails — the speculative
    // read `Parser::clause_stmt` does, spelled as a conflict.
    [$._clause, $.clause_comparison],
    [$._effect_operation_signature, $._type],
  ],

  rules: {
    source_file: $ => repeat($._statement),

    // ── Statements ────────────────────────────────────────────────────────

    /**
     * `<attribute>* <definition>`. The attributes belong to the definition
     * after them, and appear as its preceding siblings: every kind of
     * definition may carry them, so they are read here, once, rather than
     * inside each of the five. A block's `let` is read by `do_block` directly
     * and so takes none, the way `parse.rs` refuses them there.
     */
    _statement: $ => seq(
      repeat($.attribute),
      choice(
        $.extern_definition,
        $.let_definition,
        $.type_definition,
        $.effect_definition,
        $.module_definition,
      ),
    ),

    /**
     * `@key` or `@key <literal>` — one entry of a definition's metadata. The
     * value is literal data: nothing that begins a definition begins a
     * literal, so whether one follows the key is never in doubt.
     */
    attribute: $ => prec.right(seq(
      field('key', $.attribute_key),
      optional(field('value', $._data)),
    )),

    /**
     * Literal data, and only that: the scalars, a tag with an optional literal
     * payload, and tuples, arrays, and structs of these. Mirrors
     * `Parser::data` — no name, no application, no operator, no spread.
     */
    _data: $ => choice(
      $.natural,
      $.string,
      $.boolean,
      $.unit,
      $.data_tuple,
      $.data_array,
      $.data_struct,
      $.data_tag,
      $.parenthesized_data,
    ),

    parenthesized_data: $ => seq('(', $._data, ')'),

    /** `(a, b)`, or `(a,)` — a comma is what makes a tuple of one. */
    data_tuple: $ => seq(
      '(',
      field('element', $._data),
      ',',
      optional(seq(
        sepBy1(',', field('element', $._data)),
        optional(','),
      )),
      ')',
    ),

    data_array: $ => seq(
      '[',
      optional(seq(
        sepBy1(',', field('element', $._data)),
        optional(','),
      )),
      ']',
    ),

    data_struct: $ => seq(
      '{',
      optional(seq(
        sepBy1(',', $.data_field),
        optional(','),
      )),
      '}',
    ),

    data_field: $ => seq(
      field('name', fieldLabel($)),
      ':',
      field('value', $._data),
    ),

    /** `#Some 1n`, or a bare `#None` — greedy, as an expression's tag is. */
    data_tag: $ => prec.right(seq(
      field('name', $.tag),
      optional(field('payload', $._data)),
    )),

    /**
     * `module A = <stmts> end`, or `module A` for a module whose body is
     * another file.
     *
     * One rule for the two forms, told apart by whether a body was written:
     * the bare form names a file to read, and the body it stands for is
     * spliced in long before anything downstream can tell the difference. The
     * body may be empty — `module A = end` declares a module with nothing in
     * it and is legal.
     */
    module_definition: $ => seq(
      'module',
      field('name', $.identifier),
      optional(seq('=', repeat($._statement), 'end')),
    ),

    /** `extern <name> : <annotation> = <target>` — a target-provided value. */
    extern_definition: $ => seq(
      'extern',
      field('name', $.identifier),
      ':',
      field('type', $.extern_annotation),
      '=',
      field('target', $.string),
    ),

    /** The extern-only ABI type plus its ordinary outer `where` clause. */
    extern_annotation: $ => seq(
      field('type', $._extern_type),
      optional(field('clause', $.where_clause)),
    ),

    _extern_type: $ => choice(
      $.annotated_extern_type,
      $.extern_function_type,
      $.parenthesized_extern_function_type,
      $._type,
    ),

    annotated_extern_type: $ => prec.right(seq(
      repeat1($.attribute),
      field('type', choice(
        $.extern_function_type,
        $.parenthesized_extern_function_type,
        $._type,
      )),
    )),

    /** `fn(A, B) -> R [+ effects]` — one n-ary foreign-call boundary. */
    extern_function_type: $ => prec.right(seq(
      'fn',
      '(',
      optional(seq(
        field('parameter', $._extern_type),
        repeat(seq(',', field('parameter', $._extern_type))),
        optional(','),
      )),
      ')',
      '->',
      field('result', $._extern_type),
      optional(field('effects', $.effect_row)),
    )),

    /** Transparent grouping around marked or annotated boundary types. */
    parenthesized_extern_function_type: $ => seq(
      '(',
      field('type', choice(
        $.annotated_extern_type,
        $.extern_function_type,
        $.parenthesized_extern_function_type,
      )),
      ')',
    ),

    /** `let <pattern> [: <annotation>] = <expr>` */
    let_definition: $ => seq(
      'let',
      field('pattern', $._pattern),
      optional(seq(':', field('type', $.annotation))),
      '=',
      field('body', $._expression),
    ),

    /**
     * `type <name> <'param>* = <annotation>`
     *
     * The parameters are variables, and wear the `'` an annotation's variable
     * does: a bare name in a type resolves to something declared elsewhere, and
     * a parameter resolves to whatever the use site hands it.
     */
    type_definition: $ => seq(
      'type',
      field('name', $.identifier),
      repeat(field('parameter', $.type_variable)),
      '=',
      field('body', $.annotation),
    ),

    /**
     * Empty, alias, unnamed singleton, and named closed-interface effects.
     *
     * The parameters are the ones a `type` declaration binds, written the
     * same way: `effect Ask 'a = { get: () -> 'a }` takes one, and every row
     * naming `!Ask` hands it an argument.
     */
    effect_definition: $ => seq(
      'effect',
      field('name', $.identifier),
      repeat(field('parameter', $.type_variable)),
      optional(seq('=', field('body', choice(
        prec(1, $.effect_alias_union),
        $._effect_operation_signature,
        $.named_effect_interface,
      )))),
    ),

    /**
     * `!Ask 'a + !Log + ..'e` — the row an alias stands for: applications of
     * other effects, and at most one tail naming a declared parameter.
     */
    effect_alias_union: $ => $._effect_alias_row,

    _effect_alias_row: $ => choice(
      $.rest,
      prec.right(seq($.effect_alias, optional(seq('+', $._effect_alias_row)))),
    ),

    named_effect_interface: $ => seq(
      '{',
      $.effect_operation_field,
      repeat(seq(',', $.effect_operation_field)),
      optional(','),
      '}',
    ),

    effect_operation_field: $ => seq(
      field('name', $.identifier),
      ':',
      field('signature', $._effect_operation_signature),
    ),

    // Grouping around an operation arrow is transparent and may nest, but its
    // recursive base is still an arrow: `(Nat)` is not an operation signature.
    _effect_operation_signature: $ => choice(
      $.function_type,
      $.parenthesized_effect_operation_signature,
    ),

    parenthesized_effect_operation_signature: $ => seq(
      '(',
      field('signature', $._effect_operation_signature),
      ')',
    ),

    /**
     * `!Log`, `Sys::!Log`, or `!Ask 'a` — an effect this declaration stands
     * for, applied to the arguments its declaration takes.
     */
    effect_alias: $ => prec(2, seq(
      field('name', choice($.effect_label, $.effect_path)),
      repeat(field('argument', $._type_atom)),
    )),

    // ── Paths ─────────────────────────────────────────────────────────────

    // The `Math::` segments in front of a name, outermost first. One rule for
    // both kinds of path, so that reading a prefix commits to neither:
    // `Sys::Inner::!Log` and `Sys::Inner::zero` are the same up to the sigil,
    // and a parser that guessed at the first `::` would have to take the guess
    // back at the last one.
    _module_prefix: $ => repeat1(seq(field('module', $.identifier), '::')),

    /**
     * `Math::double`, `Math::Vec::zero` — a name and the modules it is reached
     * through.
     *
     * Only the qualified form is a node: a bare name is an [`identifier`] and
     * stays one, so every position that takes a path writes the choice out.
     * `::` binds tighter than application and than projection, which needs no
     * precedence to say — a path is an atom, so `Math::mk 1 2` applies
     * `Math::mk` and `Math::p.x` projects `x` out of `Math::p`.
     */
    path: $ => seq($._module_prefix, field('name', $.identifier)),

    /**
     * `Sys::!Log` — the same, for the sigilled label of an effect.
     *
     * The segments come before the `!`: the path qualifies the whole label,
     * not the name inside it.
     */
    effect_path: $ => seq($._module_prefix, field('name', $.effect_label)),

    // ── Expressions ───────────────────────────────────────────────────────

    _expression: $ => choice(
      $.match_function,
      $.function,
      $.raise_expression,
      $.assignment,
      $.pipeline,
      $._boolean_or,
    ),

    assignment: $ => prec.right(PREC.assignment, seq(
      field('target', choice($.pipeline, $._boolean_or)),
      ':=',
      field('value', $._expression),
    )),

    pipeline: $ => prec.left(PREC.pipeline, seq(
      field('value', choice($.pipeline, $._boolean_or)),
      '|>',
      field('function', $._boolean_or),
    )),

    _boolean_or: $ => choice($.boolean_or, $._boolean_xor),
    boolean_or: $ => prec.left(PREC.booleanOr, seq(
      field('left', $._boolean_or),
      'or',
      field('right', $._boolean_xor),
    )),

    _boolean_xor: $ => choice($.boolean_xor, $._boolean_and),
    boolean_xor: $ => prec.left(PREC.booleanXor, seq(
      field('left', $._boolean_xor),
      'xor',
      field('right', $._boolean_and),
    )),

    _boolean_and: $ => choice($.boolean_and, $.binary_expression),
    boolean_and: $ => prec.left(PREC.booleanAnd, seq(
      field('left', $._boolean_and),
      'and',
      field('right', $.binary_expression),
    )),

    binary_expression: $ => choice(
      $.addition,
      $.multiplication,
      $.unary_expression,
      $._application_expression,
    ),

    addition: $ => prec.left(PREC.addition, seq(
      field('left', $.binary_expression),
      field('operator', choice('+', '-')),
      field('right', $.binary_expression),
    )),

    multiplication: $ => prec.left(PREC.multiplication, seq(
      field('left', $.binary_expression),
      field('operator', choice('*', '/')),
      field('right', $.binary_expression),
    )),

    unary_expression: $ => prec(PREC.unary, seq(
      choice('-', 'not', 'mut', '~'),
      field('value', $.binary_expression),
    )),

    _application_expression: $ => choice(
      $.application,
      $._head_expression,
    ),

    /**
     * The expressions an application may be built from. An `if`, `match`,
     * `handle`, or `do` reaches here — each may be applied and projected off —
     * but none begins an argument, which is what keeps `f match ... end` from
     * being an application of `f`.
     */
    _head_expression: $ => choice(
      $._atom,
      $.projection,
      $.if_expression,
      $.match_expression,
      $.handle_expression,
      $.do_block,
    ),

    /** `f x y` — application, left-associative and ML-style. */
    application: $ => prec.left(PREC.application, seq(
      field('function', $._application_expression),
      field('argument', $._argument),
    )),

    /**
     * What may follow a function without parentheses around it: the atoms
     * `Parser::at_expr_atom` lists, whatever is projected off one of them, and
     * a `fn`, whose body then runs to the end of the application.
     */
    _argument: $ => choice(
      $._atom,
      alias($._atom_projection, $.projection),
      $.match_function,
      $.function,
    ),

    // A projection whose base is an atom rather than a `match` or a `handle`,
    // since neither of those begins an argument. The same node as
    // [`projection`], reached from the one position that narrows what it may
    // be read off.
    _atom_projection: $ => prec.left(PREC.projection, seq(
      field('base', choice($._atom, alias($._atom_projection, $.projection))),
      '.',
      field('field', choice(
        $.identifier,
        $.string,
        $.numeric_field,
        alias($._malformed_numeric_field, $.ERROR),
      )),
    )),

    _atom: $ => choice(
      $.identifier,
      $.path,
      $.natural,
      $.string,
      $.boolean,
      $.unit,
      $.struct_expression,
      $.array_expression,
      $.tuple_expression,
      $.tag_expression,
      $.operation,
      $.parenthesized_expression,
    ),

    /** `p.x` — postfix projection, left-associative. */
    projection: $ => prec.left(PREC.projection, seq(
      field('base', choice(
        $._atom,
        $.projection,
        $.if_expression,
        $.match_expression,
        $.handle_expression,
        $.do_block,
      )),
      '.',
      field('field', choice(
        $.identifier,
        $.string,
        $.numeric_field,
        alias($._malformed_numeric_field, $.ERROR),
      )),
    )),

    /** `fn <arg>+ => <expr>` — the body runs as far right as it can. */
    function: $ => prec.right(seq(
      'fn',
      repeat1(field('parameter', choice($.identifier, $.wildcard))),
      '=>',
      field('body', $._expression),
    )),

    /** `fn | <pattern> => <expr> (| <pattern> => <expr>)*`. */
    match_function: $ => prec.right(seq(
      'fn',
      '|',
      sepBy1('|', $.match_arm),
    )),

    /**
     * `do <let>* [return <expr>] end` — bindings, each in scope for the rest
     * of the block, and the value the block ends with. Only a `let` may be
     * written in a block: `parse.rs` refuses every other definition at its
     * keyword, and so does this.
     */
    do_block: $ => seq(
      'do',
      repeat(field('statement', $.let_definition)),
      optional(seq('return', field('value', $._expression))),
      'end',
    ),

    /**
     * `if <condition> then <expr> (else if <condition> then <expr>)*
     * else <expr> end`.
     *
     * An else-if chain is one expression with one final `end`; individual
     * arms deliberately have no terminator of their own.
     */
    if_expression: $ => seq(
      'if',
      field('condition', $._expression),
      'then',
      field('consequent', $._expression),
      repeat(field('else_if', $.else_if_arm)),
      'else',
      field('alternative', $._expression),
      'end',
    ),

    // Prefer closing an arm when another `else` arrives. A nested `if` in an
    // arm still owns its `else` because its required `end` makes that reading
    // unambiguous once the rest of the input is seen.
    else_if_arm: $ => prec.left(seq(
      'else',
      'if',
      field('condition', $._expression),
      'then',
      field('consequent', $._expression),
    )),

    /** `match <expr> with [|] <arm> (| <arm>)* end` */
    match_expression: $ => seq(
      'match',
      field('scrutinee', $._expression),
      'with',
      optional(seq(optional('|'), sepBy1('|', $.match_arm))),
      'end',
    ),

    match_arm: $ => seq(
      field('pattern', $._pattern),
      '=>',
      field('body', $._expression),
    ),

    /** `handle <expr> with [|] <arm> (| <arm>)* end` */
    handle_expression: $ => seq(
      'handle',
      field('body', $._expression),
      'with',
      optional(seq(optional('|'), sepBy1('|', $.handler_arm))),
      'end',
    ),

    /**
     * `!Log.write s => ...` or `return x => ...`. `return` heads an arm here
     * and is an ordinary name everywhere else.
     */
    handler_arm: $ => seq(
      field('head', choice($.operation, 'return')),
      field('binder', choice($.identifier, $.wildcard)),
      '=>',
      field('body', $._expression),
    ),

    /** `raise <expr>` — the body runs as far right as a `fn`'s does. */
    raise_expression: $ => prec.right(seq('raise', $._expression)),

    /**
     * `{ x: 1, y: 2, ..c }`, with an optional trailing comma: the fields it
     * names, then — at most once, and last — a `..` spreading every field of
     * one more value in. The same `spread` node an array literal has, since
     * it is the same `..` and the same whole-expression operand.
     */
    struct_expression: $ => seq('{', optional($._struct_items), '}'),

    _struct_items: $ => choice(
      seq($.spread, optional(',')),
      seq($.struct_field, optional(seq(',', optional($._struct_items)))),
    ),

    struct_field: $ => seq(
      field('name', fieldLabel($)),
      ':',
      field('value', $._expression),
    ),

    /** `#Some 1` — one case of a sum, with what it carries. */
    tag_expression: $ => prec.right(PREC.tag, seq(
      field('name', $.tag),
      optional(field('payload', $._argument)),
    )),

    /** `!Log`, `!State.get`, or a module-qualified form. */
    operation: $ => choice(
      prec(1, seq(
        field('effect', choice($.effect_label, $.effect_path)),
        '.',
        field('name', $.identifier),
      )),
      field('effect', choice($.effect_label, $.effect_path)),
    ),

    /** `(a, b)` — a positional struct; a singleton keeps its comma. */
    tuple_expression: $ => seq(
      '(',
      field('element', $._expression),
      ',',
      optional(seq(
        field('element', $._expression),
        repeat(seq(',', field('element', $._expression))),
        optional(','),
      )),
      ')',
    ),

    /**
     * `[a, ..b, c]` — an immutable homogeneous array, with an optional
     * trailing comma. An item is a value, or a `..` spreading another array's
     * values into the literal where it sits.
     */
    array_expression: $ => seq(
      '[',
      optional(seq(
        field('element', $._array_item),
        repeat(seq(',', field('element', $._array_item))),
        optional(','),
      )),
      ']',
    ),

    _array_item: $ => choice($.spread, $._expression),

    /** `..a` — the values of an array, spread into the literal around it. */
    spread: $ => seq('..', field('value', $._expression)),

    parenthesized_expression: $ => seq('(', $._expression, ')'),

    // ── Patterns ──────────────────────────────────────────────────────────

    _pattern: $ => choice(
      $.identifier,
      $.wildcard,
      $.natural,
      $.string,
      $.boolean,
      $.unit,
      $.struct_pattern,
      $.tuple_pattern,
      $.array_pattern,
      $.tag_pattern,
      $.parenthesized_pattern,
    ),

    /** `{ x, y: <pattern>, .. }`, with an optional trailing comma. */
    struct_pattern: $ => seq('{', optional($._struct_pattern_body), '}'),

    // The `..` comes last and takes no comma after it: the fields it stands
    // for have no order among the named ones to claim. It takes no name
    // either — a named struct rest is not part of the language yet — so the
    // struct's rest is the bare dots, shown under the one `rest_pattern` name.
    _struct_pattern_body: $ => choice(
      alias($._struct_rest, $.rest_pattern),
      seq(
        $.struct_pattern_field,
        optional(seq(',', optional($._struct_pattern_body))),
      ),
    ),

    /**
     * A field, or a bare identifier punning one to itself. Quoted and numeric
     * labels never pun: `{"field name": p}` and `{0: p}` must say which
     * pattern receives the field.
     */
    struct_pattern_field: $ => choice(
      seq(
        field('name', $.identifier),
        optional(seq(':', field('pattern', $._pattern))),
      ),
      seq(
        field('name', choice(
          $.string,
          $.numeric_field,
          alias($._malformed_numeric_field, $.ERROR),
        )),
        ':',
        field('pattern', $._pattern),
      ),
    ),

    /**
     * The `..` that makes a struct pattern match on at least its fields, or
     * an array pattern on at least its elements — where it may also name the
     * elements it stands for.
     */
    rest_pattern: $ => prec.right(seq('..', optional(field('name', $.identifier)))),

    _struct_rest: _ => '..',

    /**
     * `[a, ..rest, b]` — the elements of an array, with at most one `..`
     * anywhere among them and an optional trailing comma.
     */
    array_pattern: $ => seq(
      '[',
      optional(seq(
        field('element', $._array_pattern_item),
        repeat(seq(',', field('element', $._array_pattern_item))),
        optional(','),
      )),
      ']',
    ),

    _array_pattern_item: $ => choice($.rest_pattern, $._pattern),

    /** `#Some x` — the payload is taken greedily, as a tag expression's is. */
    tag_pattern: $ => prec.right(PREC.tag, seq(
      field('name', $.tag),
      optional(field('payload', $._pattern)),
    )),

    /** `(a, b)` — an exact positional struct pattern. */
    tuple_pattern: $ => seq(
      '(',
      field('element', $._pattern),
      ',',
      optional(seq(
        field('element', $._pattern),
        repeat(seq(',', field('element', $._pattern))),
        optional(','),
      )),
      ')',
    ),

    parenthesized_pattern: $ => seq('(', $._pattern, ')'),

    // ── Types ─────────────────────────────────────────────────────────────

    /** `<type> [where <clause> (';' <clause>)*]` — what a definition is ascribed. */
    annotation: $ => seq(
      field('type', $._type),
      optional(field('clause', $.where_clause)),
    ),

    _type: $ => choice($.function_type, $._type_sum),

    /**
     * `A -> B [+ <effects>]`, right-associative. The effect row binds to the
     * innermost arrow, so `A -> B -> C + E` performs `E` on the way from `B`
     * to `C`, and `A -> (B -> C) + E` is where the outer arrow gets one.
     */
    function_type: $ => prec.right(seq(
      field('from', $._type_sum),
      '->',
      field('to', $._type),
      optional(field('effects', $.effect_row)),
    )),

    _type_sum: $ => choice($.sum_type, $.effect_type, $._type_application),

    /**
     * `[|] <case> ('|' <case>)* ['|' ..[<rest>]]` — a sum, as the cases it
     * allows. `|` alone is the sum with no cases at all.
     */
    sum_type: $ => choice(
      seq('|', optional($._sum_body)),
      $._sum_cases,
    ),

    // A row that is nothing but its tail is read as an effect row unless a
    // leading `|` says otherwise, which is why only this branch reaches `rest`
    // with no case in front of it.
    _sum_body: $ => choice($.rest, $._sum_cases),

    // Right-associative, so a sum written as an operation's signature takes
    // every `|` after it — `effect E = op : #A | #B` declares one operation
    // returning a two-case sum, exactly as `Parser::type_sum` reads it.
    _sum_cases: $ => prec.right(seq(
      choice($.sum_case, $.absent_case),
      optional(seq('|', $._sum_body)),
    )),

    /** `#Some (when 'a) T` — a case a value may be, carrying this when it is. */
    sum_case: $ => seq(
      field('name', $.tag),
      optional(field('when', $.parenthesized_when)),
      optional(field('payload', $._type_atom)),
    ),

    /** `\#None` — the case is definitely not there. */
    absent_case: $ => seq('\\', field('name', $.tag)),

    /**
     * `!Log + !IO` — a row of effects written where a type goes. Not a type;
     * the one position that takes it is an argument at a parameter a
     * declaration uses as its effects.
     */
    effect_type: $ => $._effect_row_body,

    /** `+ !Log + !IO`, or `+ |` for the row that allows nothing. */
    effect_row: $ => seq('+', choice('|', $._effect_row_body)),

    _effect_row_body: $ => choice($.rest, $._effect_row_cases),

    // Right-associative, so a row written as an arrow's result takes every `+`
    // after it: `A -> !Log + !IO` hands the parameter both effects rather than
    // hanging the second one off the arrow.
    _effect_row_cases: $ => prec.right(seq(
      choice($.effect_case, $.absent_effect),
      optional(seq('+', $._effect_row_body)),
    )),

    /**
     * `!Log`, `!Ask Nat`, or `!Log (when 'a)` — an effect the arrow may
     * perform, applied to the arguments its declaration takes. The arguments
     * are gathered the way a type application's are: one atom each, and a
     * `(when` after the label is its presence clause rather than an argument.
     */
    effect_case: $ => seq(
      field('name', choice($.effect_label, $.effect_path)),
      repeat(field('argument', $._type_atom)),
      optional(field('when', $.parenthesized_when)),
    ),

    /** `\!Log`, or `\!Ask Nat` — the effect is definitely not performed. */
    absent_effect: $ => seq(
      '\\',
      field('name', choice($.effect_label, $.effect_path)),
      repeat(field('argument', $._type_atom)),
    ),

    /** `Pair Nat Nat` — a type applied to arguments, gathered flat. */
    _type_application: $ => choice($.type_application, $._type_atom),

    type_application: $ => prec.left(seq(
      field('head', $._type_atom),
      repeat1(field('argument', $._type_atom)),
    )),

    _type_atom: $ => choice(
      $.identifier,
      $.path,
      $.type_variable,
      $.hole,
      $.unit,
      $.struct_type,
      $.array_type,
      $.mut_type,
      $.tuple_type,
      $.parenthesized_type,
    ),

    /** `{ x: Nat, y when 'a: Nat, \z, ..'r }`, with an optional trailing comma. */
    struct_type: $ => seq('{', optional($._struct_type_body), '}'),

    _struct_type_body: $ => choice(
      $.rest,
      seq(
        choice($.struct_type_field, $.absent_field),
        optional(seq(',', optional($._struct_type_body))),
      ),
    ),

    struct_type_field: $ => seq(
      field('name', fieldLabel($)),
      optional(field('when', $.when_clause)),
      ':',
      field('type', $._type),
    ),

    /** `\name` — the label is definitely not there. */
    absent_field: $ => seq('\\', field('name', fieldLabel($))),

    /**
     * `..` or `..'r` — what is known about the labels not written out. Bare it
     * is anything at all; named it is a variable, which is a declaration's
     * parameter inside a `type` body and one of the annotation's own outside.
     */
    rest: $ => seq('..', optional(field('name', $.type_variable))),

    /** `when 'a`, or the `when _` that no formula may name. */
    when_clause: $ => seq('when', field('name', choice($.type_variable, $.wildcard))),

    /** The same clause where there is no colon to end it: `(when 'a)`. */
    parenthesized_when: $ => seq('(', $.when_clause, ')'),

    /** `(A, B)` — a closed positional struct type. */
    tuple_type: $ => seq(
      '(',
      field('element', $._type),
      ',',
      optional(seq(
        field('element', $._type),
        repeat(seq(',', field('element', $._type))),
        optional(','),
      )),
      ')',
    ),

    /** `[T]` — the type of immutable homogeneous arrays of `T`. */
    mut_type: $ => seq('mut', field('region', $._type_atom), field('element', $._type_atom)),

    array_type: $ => seq('[', field('element', $._type), ']'),

    parenthesized_type: $ => seq('(', $._type, ')'),

    // ── `where` clauses ───────────────────────────────────────────────────

    where_clause: $ => seq('where', sepBy1(';', $._clause)),

    _clause: $ => choice($.clause_comparison, $._clause_or),

    /** `a = b` or `a != b`. Non-associative: `a = b = c` has no reading. */
    clause_comparison: $ => seq(
      field('left', $._clause_or),
      field('operator', choice('=', '!=')),
      field('right', $._clause_or),
    ),

    _clause_or: $ => choice($.clause_or, $._clause_and),
    clause_or: $ => prec.left(seq($._clause_or, 'or', $._clause_and)),

    _clause_and: $ => choice($.clause_and, $._clause_not),
    clause_and: $ => prec.left(seq($._clause_and, 'and', $._clause_not)),

    _clause_not: $ => choice($.clause_not, $._clause_atom),
    clause_not: $ => prec.right(seq('not', $._clause_not)),

    _clause_atom: $ => choice(
      $.type_variable,
      $.parenthesized_clause,
    ),

    parenthesized_clause: $ => seq('(', $._clause, ')'),

    // ── Tokens ────────────────────────────────────────────────────────────

    /** `()` — the unit value, the unit pattern, and the unit type. */
    unit: _ => seq('(', ')'),

    /** `_` — a value being thrown away. */
    wildcard: _ => '_',

    /** `_` in a type: a position left for inference to decide. */
    hole: _ => '_',

    identifier: _ => new RegExp(IDENT.source, 'u'),

    /**
     * `#Some` or `#"some case"` — one case of a sum, named. A quoted tag is
     * one token so whitespace may not separate its `#` from the opening quote.
     */
    tag: _ => choice(
      new RegExp(TAG.source, 'u'),
      token(seq('#', '"', repeat(choice(/[^"\\\n]/, /\\["\\nrt]/)), '"')),
    ),

    /** `!Log` — an effect, named. The `!` is not part of the name. */
    effect_label: _ => new RegExp(EFFECT.source, 'u'),

    /** `@deprecated` — an attribute's key. The `@` is not part of the name. */
    attribute_key: _ => new RegExp(ATTRIBUTE.source, 'u'),

    /**
     * `'a` — a variable: the parameter of the declaration it is written in, or
     * a variable of the annotation it is written in.
     */
    type_variable: _ => new RegExp(VARIABLE.source, 'u'),

    /**
     * A numeric literal: suffixless reals (including decimals), or 64-bit
     * integers and naturals marked with `i` and `n`. The broad trailing word
     * is intentional: the lexer diagnoses `1x` as one malformed literal.
     */
    natural: _ => new RegExp(/[0-9]+(?:\.[0-9]+)?[\p{Alphabetic}\p{N}_]*/.source, 'u'),

    /** A decimal positional field in a projection or structural label. */
    numeric_field: _ => /[0-9]+/,

    /**
     * A number-shaped field that the compiler consumes as one invalid token.
     * Besides suffixes such as `.0n` and `.0²`, this includes an adjacent
     * decimal tail: `p.0.0` is one malformed field, while `(p.0).0` is two
     * valid projections. The set subtraction keeps another ASCII digit in the
     * valid field token, so `.001` is not mistaken for a malformed suffix.
     */
    _malformed_numeric_field: _ => new RustRegex(
      String.raw`[0-9]+(?:\.[0-9]+[\p{Alphabetic}\p{N}_]*|(?:[\p{Alphabetic}_]|[\p{N}&&[^0-9]])[\p{Alphabetic}\p{N}_]*)`,
    ),

    /**
     * A double-quoted UTF-8 string with the escapes token::lex accepts, or a
     * raw string: one or more `\\` lines, each running to the end of its line
     * with nothing escaped, joined by the whitespace between them alone.
     * Mirrors `token::raw_string` — one token, because the lexer makes one:
     * the whitespace between the lines is inside it, not an extra.
     */
    string: _ => token(choice(
      seq('"', repeat(choice(/[^"\\]/, /\\["\\nrt]/)), '"'),
      seq(/\\\\[^\n\r]*/, repeat(seq(/\s+/, /\\\\[^\n\r]*/))),
    )),

    /** The two boolean literals, reserved by token::lex. */
    boolean: _ => choice('true', 'false'),

    /**
     * `--`, running to the end of its line. Mirrors `token::line_comment`.
     */
    line_comment: _ => token(seq('--', /[^\n]*/)),

    /**
     * `(* ... *)`. May span several lines, and nests: a `(*` written inside
     * one reopens the count, and only the `*)` matching it closes that
     * nesting rather than the outer comment. Mirrors `token::block_comment`,
     * which is why this is a rule rather than a `token()` — the nesting is
     * not a regular language.
     *
     * The body is "any one character", read one at a time rather than by a
     * lookahead regex — this grammar's regex engine has none. That is enough:
     * wherever `(*` or `*)` can instead be read as the longer two-character
     * lexeme, the generated lexer's longest-match rule reads it that way, the
     * same one character at a time `token::block_comment` decides with.
     */
    block_comment: $ => seq(
      '(*',
      repeat(choice(
        $.block_comment,
        /[\s\S]/,
      )),
      '*)',
    ),
  },
});
