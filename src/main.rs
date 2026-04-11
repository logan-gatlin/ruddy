use ruddy::{Eng, Source, check_text_fs};

fn main() -> std::io::Result<()> {
    let path = "demo.hc";
    let contents = std::fs::read_to_string(path)?;

    let db = Eng::default();
    let source = Source::new(&db, path.to_owned(), contents);
    let checked = check_text_fs(&db, source);

    println!("{checked:#?}");
    Ok(())
}
