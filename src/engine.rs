#[salsa::input(debug)]
pub struct Source {
    #[returns(ref)]
    pub name: String,
    #[returns(ref)]
    pub contents: String,
}

/// The query engine
#[salsa::db]
#[derive(Clone, Default)]
pub struct Eng {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Eng {}
