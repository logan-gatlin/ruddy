use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::resolver::Resolver;

use super::*;

pub(super) fn resolver_token<'db, R: Resolver>(db: &'db dyn salsa::Database) -> ResolverToken<'db> {
    let key = std::any::type_name::<R>().to_owned();
    let dispatch = ResolverDispatch {
        canonize_bare: R::canonize_bare,
        canonize: R::canonize,
        resolve: R::resolve,
    };

    let registry = resolver_registry();
    let mut registry = registry.lock().expect("resolver registry lock poisoned");
    registry.entry(key.clone()).or_insert(dispatch);

    ResolverToken::new(db, key)
}

pub(super) fn resolver_dispatch(key: &str) -> Option<ResolverDispatch> {
    let registry = resolver_registry();
    let registry = registry.lock().ok()?;
    registry.get(key).copied()
}

fn resolver_registry() -> &'static Mutex<HashMap<String, ResolverDispatch>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, ResolverDispatch>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}
