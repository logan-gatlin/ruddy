use ruddy::{
    inference, ir, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileID,
};

fn main() {
    let src = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let lexed = token::lex(&src, FileID::GENERATED);
    let parsed = parse::parse(lexed.tokens);
    println!("lex errors: {:?}", lexed.errors.len());
    for e in &parsed.errors {
        println!("parse error: {e:?}");
    }
    let bundle = Bundle::new("probe", Version::new(0, 1, 0)).unwrap();
    let mut mint = Mint::new(bundle);
    let mut built = ir::build(&mut mint, parsed.stmts);
    for e in &built.errors {
        println!("ir error: {} [{}] @ {:?}", e.kind, e.kind.code(), e.span);
    }
    let inferred = inference::infer(&mint, &mut built.program);
    for e in &inferred.errors {
        println!("infer error: {} [{}] @ {:?}", e.kind, e.kind.code(), e.span);
    }
    for (s, sc) in &inferred.schemes {
        println!("scheme {} : {}", mint.name(*s), sc);
    }
    for (s, sc) in &inferred.aliases {
        println!("alias {} : {}", mint.name(*s), sc);
    }
}
