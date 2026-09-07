use std::{fs, process::Command};

use ruddy::{
    artifact::Artifact,
    backend::js,
    compile, inference, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileID,
    types::{FixedInt, FixedLiteral, Prim},
};

fn compile_source(
    source: &str,
) -> Result<compile::AcceptedProgram, Box<compile::PartialCompilation>> {
    let lexed = token::lex(source, FileID::GENERATED);
    assert!(lexed.errors.is_empty(), "{:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let bundle = Bundle::new("fixed", Version::new(1, 0, 0)).unwrap();
    compile::compile(Mint::new(bundle), parsed.stmts, inference::Trace::Off).map_err(Box::new)
}

#[test]
fn fixed_integer_literals_check_every_width_and_signed_boundary() {
    for kind in FixedInt::ALL {
        assert_eq!(Prim::from_name(kind.name()), Some(Prim::Fixed(kind)));
        for value in [kind.min(), 0, kind.max()] {
            let literal = format!("{value}{}", kind.suffix());
            let lexed = token::lex(&literal, FileID::GENERATED);
            assert!(lexed.errors.is_empty(), "{literal}: {:?}", lexed.errors);
            assert!(matches!(lexed.tokens[0].tracked, token::Kind::Fixed(found)
                if found == FixedLiteral::new(kind, value).unwrap()));
            assert_eq!(lexed.tokens[0].tracked.to_string(), literal);
            assert_eq!(lexed.tokens[0].span.width, literal.len());
        }
        for value in [kind.min() - 1, kind.max() + 1] {
            let literal = format!("{value}{}", kind.suffix());
            let lexed = token::lex(&literal, FileID::GENERATED);
            assert_eq!(lexed.errors.len(), 1, "{literal}: {:?}", lexed.errors);
            assert_eq!(
                lexed.errors[0].kind,
                if value < 0 && !kind.signed() {
                    token::ErrorKind::NegativeNatural
                } else {
                    token::ErrorKind::FixedOutOfRange { kind }
                }
            );
        }
        for literal in [
            format!("1.5{}", kind.suffix()),
            format!("1{}name", kind.suffix()),
        ] {
            assert_eq!(
                token::lex(&literal, FileID::GENERATED).errors.len(),
                1,
                "{literal}"
            );
        }
    }
    assert_eq!(token::lex("1n128", FileID::GENERATED).errors.len(), 1);
}

#[test]
fn fixed_integer_types_remain_distinct_and_round_trip_with_literals_and_metadata() {
    for kind in FixedInt::ALL {
        let source = format!(
            "@limit {max}{suffix}\ntype Value = {ty}\nlet id : Value -> Value = fn value => value\nlet value = id {max}{suffix}\nlet lowest : {ty} = {min}{suffix}\nlet selected = match value with | {max}{suffix} => true | _ => false end\n",
            max = kind.max(),
            min = kind.min(),
            suffix = kind.suffix(),
            ty = kind.name(),
        );
        let program = compile_source(&source).unwrap_or_else(|e| panic!("{source}\n{e:#?}"));
        let artifact = program.artifact();
        let printed = artifact.print();
        let parsed = Artifact::try_parse(&printed).unwrap().validate().unwrap();
        assert_eq!(&parsed, artifact);
        assert_eq!(parsed.print(), printed);
        let javascript = js::generate(&parsed).unwrap();
        assert!(javascript.contains(&format!(
            "{}{}",
            kind.max(),
            if kind.bits() == 64 { "n" } else { "" }
        )));
        for other in FixedInt::ALL.into_iter().filter(|other| *other != kind) {
            let source = format!("let value : {} = 0{}", kind.name(), other.suffix());
            assert!(compile_source(&source).is_err(), "{source}");
        }
        for other in ["n", "i", ""] {
            let source = format!("let value : {} = 0{other}", kind.name());
            assert!(compile_source(&source).is_err(), "{source}");
        }
    }
}

#[test]
fn fixed_integer_patterns_cover_finite_domains_and_reject_missing_or_duplicate_values() {
    for kind in [FixedInt::Nat8, FixedInt::Int8] {
        let arms: Vec<_> = (kind.min()..=kind.max())
            .map(|value| format!("| {value}{} => {value}", kind.suffix()))
            .collect();
        let source = format!(
            "let covered = fn value => match value with {} end",
            arms.join("\n")
        );
        let program = compile_source(&source).unwrap_or_else(|e| panic!("{e:#?}"));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("complete.mjs");
        fs::write(&path, js::generate(program.artifact()).unwrap()).unwrap();
        let probe = format!(
            "import assert from 'node:assert/strict'; const x = await import({}); for (let v = {}; v <= {}; v++) assert.equal(x.covered(v), v);",
            serde_json::to_string(path.to_str().unwrap()).unwrap(),
            kind.min(),
            kind.max()
        );
        let output = Command::new("node")
            .args(["--input-type=module", "--eval", &probe])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let missing = format!(
            "let missing = fn value => match value with {} end",
            arms[1..].join("\n")
        );
        assert!(compile_source(&missing).is_err());
        let duplicate = format!(
            "let duplicate = fn value => match value with {} {} end",
            arms.join("\n"),
            arms[0]
        );
        assert!(compile_source(&duplicate).is_err());
    }
}

#[test]
fn fixed_integer_artifacts_reject_out_of_range_literal_payloads() {
    let literal = FixedLiteral::new(FixedInt::Nat8, 255).unwrap();
    let mut json = serde_json::to_value(literal).unwrap();
    json["value"] = serde_json::json!(256);
    assert!(serde_json::from_value::<FixedLiteral>(json).is_err());
    let program = compile_source("@limit 255n8\nlet value = 255n8").unwrap();
    let text = program.artifact().print();
    assert!(text.contains("(fixed n8 255)"), "{text}");
    assert!(Artifact::try_parse(&text.replace("(fixed n8 255)", "(fixed n8 256)")).is_err());
}

#[test]
fn fixed_integer_std_arithmetic_runs_exactly_through_imports_and_javascript() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("Ruddy.toml"), format!(
        "name = \"fixed-test\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"main.hc\"\ntarget = \"js\"\n[dependencies]\nstd = {:?}\n", root.join("std")
    )).unwrap();
    let mut source = String::new();
    let mut assertions = Vec::new();
    for kind in FixedInt::ALL {
        let module = if kind.signed() { "int" } else { "nat" };
        let bits = kind.bits();
        let suffix = kind.suffix();
        let max = kind.max();
        let min = kind.min();
        let prefix = format!("std::{module}");
        let fields = [
            (
                "add",
                format!("{prefix}::add{bits} {max}{suffix} 1{suffix}"),
                min,
            ),
            (
                "sub",
                format!("{prefix}::subtract{bits} {min}{suffix} 1{suffix}"),
                max,
            ),
            (
                "mul",
                format!("{prefix}::multiply{bits} {max}{suffix} {max}{suffix}"),
                1,
            ),
            (
                "div",
                format!("{prefix}::divide{bits} 7{suffix} 2{suffix}"),
                3,
            ),
            (
                "rem",
                format!("{prefix}::remainder{bits} 7{suffix} 2{suffix}"),
                1,
            ),
            ("max", format!("{prefix}::max_value{bits}"), max),
            ("min", format!("{prefix}::min_value{bits}"), min),
        ];
        for (op, expression, expected) in fields {
            let name = format!("{module}{bits}_{op}");
            source.push_str(&format!("let {name} = {expression}\n"));
            assertions.push(format!("assert.equal(String(x.{name}), '{expected}');"));
            assertions.push(format!(
                "assert.equal(typeof x.{name}, '{}');",
                if bits == 64 { "bigint" } else { "number" }
            ));
        }
        source.push_str(&format!("let {module}{bits}_match = match {max}{suffix} with | {max}{suffix} => true | _ => false end\n"));
        assertions.push(format!("assert.equal(x.{module}{bits}_match, true);"));
        source.push_str(&format!("let {module}{bits}_divide = {prefix}::divide{bits}\nlet {module}{bits}_remainder = {prefix}::remainder{bits}\n"));
        let zero = if bits == 64 { "0n" } else { "0" };
        assertions.push(format!(
            "assert.throws(() => x.{module}{bits}_divide({zero})({zero}), RangeError);"
        ));
        assertions.push(format!(
            "assert.throws(() => x.{module}{bits}_remainder({zero})({zero}), RangeError);"
        ));
        if kind.signed() {
            for (name, expr, expected) in [
                (
                    "negative_div",
                    format!("{prefix}::divide{bits} -7{suffix} 2{suffix}"),
                    -3,
                ),
                (
                    "negative_rem",
                    format!("{prefix}::remainder{bits} -7{suffix} 2{suffix}"),
                    -1,
                ),
                (
                    "overflow_div",
                    format!("{prefix}::divide{bits} {min}{suffix} -1{suffix}"),
                    min,
                ),
                (
                    "negate",
                    format!("{prefix}::negate{bits} {min}{suffix}"),
                    min,
                ),
                ("abs", format!("{prefix}::abs{bits} {min}{suffix}"), min),
            ] {
                source.push_str(&format!("let int{bits}_{name} = {expr}\n"));
                assertions.push(format!(
                    "assert.equal(String(x.int{bits}_{name}), '{expected}');"
                ));
            }
        }
    }
    source.push_str("let exact64 = std::nat::subtract64 9007199254740993n64 9007199254740992n64\nlet shown64 = std::nat::to_string64 18446744073709551615n64\nlet narrow = std::int::from_int8 255i\nlet wide = std::int::to_int8 -128i8\nlet to_nat64 = std::nat::to_nat64\nlet kept = (fn x => x) [9007199254740993n64]\nlet kept_first = match kept with | [value] => value | _ => 0n64 end\n");
    assertions.push("assert.equal(x.kept_first, 9007199254740993n); assert.equal(x.exact64, 1n); assert.equal(x.shown64, '18446744073709551615'); assert.equal(x.narrow, -1); assert.equal(x.wide, -128); assert.throws(() => x.to_nat64(18446744073709551615n), RangeError);".into());
    fs::write(project.path().join("main.hc"), source).unwrap();
    let artifact =
        ruddy_cli::build_project(project.path()).expect("fixed-width std consumer builds");
    let javascript = artifact.with_extension("js");
    assert!(javascript.is_file());
    let probe = format!(
        "import assert from 'node:assert/strict'; import {{ pathToFileURL }} from 'node:url'; const x = await import(pathToFileURL({}).href); {}",
        serde_json::to_string(javascript.to_str().unwrap()).unwrap(),
        assertions.join("\n")
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .expect("Node is required for fixed-width runtime checks");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
