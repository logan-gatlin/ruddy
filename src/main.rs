use ruddy::{Eng, Source, parse_text};

fn main() -> std::io::Result<()> {
    let path = "demo.hc";
    let contents = std::fs::read_to_string(path)?;

    let db = Eng::default();
    let source = Source::new(&db, path.to_owned(), contents);
    let parsed = parse_text(&db, source);

    println!("{parsed:#?}");
    Ok(())
}
