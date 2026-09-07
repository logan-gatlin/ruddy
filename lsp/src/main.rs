fn main() {
    let (connection, threads) = lsp_server::Connection::stdio();
    if let Err(error) = ruddy_lsp::serve(connection) {
        eprintln!("ruddy-ls: {error}");
        std::process::exit(1);
    }
    if let Err(error) = threads.join() {
        eprintln!("ruddy-ls: {error}");
        std::process::exit(1);
    }
}
