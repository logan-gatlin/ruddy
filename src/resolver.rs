use std::path::Path;

pub trait Resolver {
    /// Canonize `module <name>` into a fully qualified source name.
    /// `from` is the source where this name was referenced
    fn canonize_bare(name: &str, from: &str) -> Option<String>;
    /// Canonize `module _ in "<name>"` into a fully qualified source name
    /// `from` is the source where this name was referenced
    fn canonize(name: &str, from: &str) -> Option<String>;
    /// Resolve a canonized source name into the source's contents
    fn resolve(canon: &str) -> Option<String>;
}

#[derive(Clone, Copy)]
pub struct ResolverDispatch {
    pub canonize_bare: fn(&str, &str) -> Option<String>,
    pub canonize: fn(&str, &str) -> Option<String>,
    pub resolve: fn(&str) -> Option<String>,
}

#[salsa::interned(debug)]
pub struct ResolverToken<'db> {
    #[returns(ref)]
    pub key: String,
}

pub struct FilesystemResolver;

impl Resolver for FilesystemResolver {
    fn canonize_bare(name: &str, from: &str) -> Option<String> {
        let source_name = format!("{name}.hc");
        Self::canonize(&source_name, from)
    }

    fn canonize(name: &str, from: &str) -> Option<String> {
        let name: &str = name;
        let name = Path::new(name);
        let candidate = if name.is_absolute() {
            name.to_path_buf()
        } else {
            let from = Path::new(from);
            from.parent().unwrap_or(from).join(name)
        };

        std::fs::canonicalize(candidate)
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    }

    fn resolve(canon: &str) -> Option<String> {
        std::fs::read_to_string(canon).ok()
    }
}

pub struct FailingResolver;

impl Resolver for FailingResolver {
    fn canonize_bare(_name: &str, _from: &str) -> Option<String> {
        None
    }

    fn canonize(_name: &str, _from: &str) -> Option<String> {
        None
    }

    fn resolve(_canon: &str) -> Option<String> {
        None
    }
}
