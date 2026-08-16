/**
 * @file Tree-sitter grammar for ruddy (`.hc`)
 *
 * Mirrors `src/token.rs` and `src/parse.rs`. Where the two could disagree,
 * `src/parse.rs` is the source of truth: every rule below is named after the
 * production it stands for there.
 *
 * The language has no comments, so `extras` is whitespace and nothing else.
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

const PREC = {
  // `f x y` groups to the left, and stops in front of anything that begins no
  // argument.
  application: 1,
  // A tag takes its payload before an application takes another argument, so
  // `f #A 1` is `f` applied to `#A 1`.
  tag: 2,
  // `f p.x` reaches into the record before passing it along.
  projection: 3,
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

module.exports = grammar({
  name: 'ruddy',

  word: $ => $.identifier,

  // The words `token::lex` reserves: no position reads one of these as a name.
  // `when`, `where`, `or`, `and`, `not` and `return` are deliberately absent —
  // they are contextual, read by spelling at the few positions that want them
  // and ordinary identifiers everywhere else.
  reserved: {
    global: _ => [
      'let',
      'in',
      'type',
      'end',
      'with',
      'match',
      'fn',
      'effect',
      'handle',
      'raise',
    ],
  },

  extras: _ => [/\s/],

  conflicts: $ => [
    // `where 'a = 'b = v` compares two presences and then defines `v`;
    // `where 'a = v` is the clause `'a` and then the definition's own `=`.
    // Which one an `=` is cannot be known until the value after it is read,
    // so both readings are carried until one of them fails — the speculative
    // read `Parser::clause_stmt` does, spelled as a conflict.
    [$._clause, $.clause_comparison],
  ],

  rules: {
    source_file: $ => repeat($._statement),

    // ── Statements ────────────────────────────────────────────────────────

    _statement: $ => choice(
      $.let_definition,
      $.type_definition,
      $.effect_definition,
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
     * `effect <name> = [|] <case> ('|' <case>)*`, where a case is an operation
     * and its signature or an effect this one stands for. Aliases may also be
     * joined with the `+` an effect row writes; operations may not.
     */
    effect_definition: $ => seq(
      'effect',
      field('name', $.identifier),
      '=',
      optional('|'),
      optional($._effect_cases),
    ),

    _effect_cases: $ => choice(
      seq($.operation_declaration, optional(seq('|', $._effect_cases))),
      seq($.effect_alias, optional(seq(choice('|', '+'), $._effect_cases))),
    ),

    /** `write : Nat -> ()` — an operation and the signature performing it has. */
    operation_declaration: $ => seq(
      field('name', $.identifier),
      ':',
      field('signature', $._type),
    ),

    /** `!Log` — an effect this declaration stands for. */
    effect_alias: $ => $.effect_label,

    // ── Expressions ───────────────────────────────────────────────────────

    _expression: $ => choice(
      $.function,
      $.let_expression,
      $.raise_expression,
      $._application_expression,
    ),

    _application_expression: $ => choice(
      $.application,
      $._head_expression,
    ),

    /**
     * The expressions an application may be built from. A `match` or a
     * `handle` reaches here — both may be applied and projected off — but
     * neither begins an argument, which is what keeps `f match ... end` from
     * being an application of `f`.
     */
    _head_expression: $ => choice(
      $._atom,
      $.projection,
      $.match_expression,
      $.handle_expression,
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
      $.function,
    ),

    // A projection whose base is an atom rather than a `match` or a `handle`,
    // since neither of those begins an argument. The same node as
    // [`projection`], reached from the one position that narrows what it may
    // be read off.
    _atom_projection: $ => prec.left(PREC.projection, seq(
      field('base', choice($._atom, alias($._atom_projection, $.projection))),
      '.',
      field('field', $.identifier),
    )),

    _atom: $ => choice(
      $.identifier,
      $.natural,
      $.unit,
      $.struct_expression,
      $.tag_expression,
      $.operation,
      $.parenthesized_expression,
    ),

    /** `p.x` — postfix projection, left-associative. */
    projection: $ => prec.left(PREC.projection, seq(
      field('base', choice(
        $._atom,
        $.projection,
        $.match_expression,
        $.handle_expression,
      )),
      '.',
      field('field', $.identifier),
    )),

    /** `fn <arg>+ => <expr>` — the body runs as far right as it can. */
    function: $ => prec.right(seq(
      'fn',
      repeat1(field('parameter', choice($.identifier, $.wildcard))),
      '=>',
      field('body', $._expression),
    )),

    /** `let <pattern> [: <annotation>] = <value> in <body>` */
    let_expression: $ => prec.right(seq(
      'let',
      field('pattern', $._pattern),
      optional(seq(':', field('type', $.annotation))),
      '=',
      field('value', $._expression),
      'in',
      field('body', $._expression),
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

    /** `{ x: 1, y: 2 }`, with an optional trailing comma. */
    struct_expression: $ => seq('{', optional($._struct_fields), '}'),

    _struct_fields: $ => seq(
      $.struct_field,
      optional(seq(',', optional($._struct_fields))),
    ),

    struct_field: $ => seq(
      field('name', $.identifier),
      ':',
      field('value', $._expression),
    ),

    /** `#Some 1` — one case of a sum, with what it carries. */
    tag_expression: $ => prec.right(PREC.tag, seq(
      field('name', $.tag),
      optional(field('payload', $._argument)),
    )),

    /** `!Log.write` — one operation of an effect, as an ordinary value. */
    operation: $ => seq(
      field('effect', $.effect_label),
      '.',
      field('name', $.identifier),
    ),

    parenthesized_expression: $ => seq('(', $._expression, ')'),

    // ── Patterns ──────────────────────────────────────────────────────────

    _pattern: $ => choice(
      $.identifier,
      $.wildcard,
      $.natural,
      $.unit,
      $.struct_pattern,
      $.tag_pattern,
      $.parenthesized_pattern,
    ),

    /** `{ x, y: <pattern>, .. }`, with an optional trailing comma. */
    struct_pattern: $ => seq('{', optional($._struct_pattern_body), '}'),

    // The `..` comes last and takes no comma after it: the fields it stands
    // for have no order among the named ones to claim.
    _struct_pattern_body: $ => choice(
      $.rest_pattern,
      seq(
        $.struct_pattern_field,
        optional(seq(',', optional($._struct_pattern_body))),
      ),
    ),

    /** A field, or a bare name punning one to itself. */
    struct_pattern_field: $ => seq(
      field('name', $.identifier),
      optional(seq(':', field('pattern', $._pattern))),
    ),

    /** The `..` that makes a struct pattern match on at least its fields. */
    rest_pattern: _ => '..',

    /** `#Some x` — the payload is taken greedily, as a tag expression's is. */
    tag_pattern: $ => prec.right(PREC.tag, seq(
      field('name', $.tag),
      optional(field('payload', $._pattern)),
    )),

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

    /** `!Log`, or `!Log (when 'a)` — an effect the arrow may perform. */
    effect_case: $ => seq(
      field('name', $.effect_label),
      optional(field('when', $.parenthesized_when)),
    ),

    /** `\!Log` — the effect is definitely not performed. */
    absent_effect: $ => seq('\\', field('name', $.effect_label)),

    /** `Pair Nat Nat` — a type applied to arguments, gathered flat. */
    _type_application: $ => choice($.type_application, $._type_atom),

    type_application: $ => prec.left(seq(
      field('head', $._type_atom),
      repeat1(field('argument', $._type_atom)),
    )),

    _type_atom: $ => choice(
      $.identifier,
      $.type_variable,
      $.hole,
      $.unit,
      $.struct_type,
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
      field('name', $.identifier),
      optional(field('when', $.when_clause)),
      ':',
      field('type', $._type),
    ),

    /** `\name` — the label is definitely not there. */
    absent_field: $ => seq('\\', field('name', $.identifier)),

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

    /** `#Some` — one case of a sum, named. The `#` is not part of the name. */
    tag: _ => new RegExp(TAG.source, 'u'),

    /** `!Log` — an effect, named. The `!` is not part of the name. */
    effect_label: _ => new RegExp(EFFECT.source, 'u'),

    /**
     * `'a` — a variable: the parameter of the declaration it is written in, or
     * a variable of the annotation it is written in.
     */
    type_variable: _ => new RegExp(VARIABLE.source, 'u'),

    /**
     * A natural number literal.
     *
     * Runs over the characters an identifier continues with, not just digits,
     * exactly as `token::lex` reads one: `1x` is a single broken literal
     * rather than a `1` beside an `x`. Whether the digits are digits — and
     * whether they fit in the `u128` the compiler holds them in — is the
     * lexer's to say, so both `1x` and a number too large parse here and are
     * refused there.
     */
    natural: _ => new RegExp(/[0-9][\p{Alphabetic}\p{N}_]*/.source, 'u'),
  },
});
