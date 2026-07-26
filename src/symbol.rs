use std::{collections::HashMap, fmt, fmt::Write as _};

use indexmap::IndexSet;
use semver::Prerelease;
use twox_hash::XxHash3_64;

pub use semver::Version;

/// Marks a string as one of our mangled names. Also guarantees the mangling
/// starts with a character that is legal at the front of an identifier.
pub const PREFIX: &str = "_R";

/// Tags introducing the parts of a mangled name that come before the path.
/// All lowercase, and so disjoint from the uppercase namespace tags.
const BUNDLE_TAG: char = 'B';
const MAJOR_TAG: char = 'v';
const MINOR_TAG: char = 'm';
const PATCH_TAG: char = 'p';
const PRERELEASE_TAG: char = 'r';

/// A bundle's identity: an ASCII name and a version.
///
/// Nothing here is assigned by a registry, so the same bundle is the same
/// value in every run and every tool. This pair is the sole reason two
/// bundles' manglings cannot collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Bundle {
    name: String,
    version: Version,
    hash: BundleHash,
}

/// A fingerprint of a [`Bundle`], so that a [`Symbol`] can name its bundle in
/// eight bytes instead of carrying a name and version around.
///
/// Derived from the bundle's identity rather than handed out by a counter, so
/// it is stable across runs. Distinct bundles that fingerprint the same would
/// be a correctness bug; [`Bundles::register`] is where that is ruled out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BundleHash(u64);

/// A globally unique name for one variable, type, or module.
///
/// A symbol is an identity, not a name: equality is identity, and everything
/// else about it — its name, namespace, and containing module — is held by the
/// [`Mint`] that made it. Symbols carry the bundle they were minted in so that
/// two bundles' symbols can never compare equal by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Symbol {
    bundle: BundleHash,
    index: u32,
}

/// A symbol already known to live in the module namespace. Newtyped so that a
/// term can never be passed where a containing module is expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Module(Symbol);

/// The three disjoint worlds a symbol can live in. The same name in two
/// namespaces makes two unrelated symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    Terms,
    Types,
    Modules,
}

/// Index into the mint's interned name table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Name(u32);

/// Everything the mint knows about one symbol. Spans are deliberately absent;
/// callers keep a [`Tracked<Symbol>`](crate::tracking::Tracked) instead, since
/// one symbol can be written at many places.
#[derive(Debug, Clone)]
struct Data {
    name: Name,
    namespace: Namespace,
    /// `None` places the symbol directly at the top level of the bundle, which
    /// is a real position in the tree rather than a missing one.
    parent: Option<Module>,
    /// Present when the symbol is local.
    disambiguator: Option<u32>,
}

/// Builder for unique symbols.
///
/// One mint per bundle: the mint *is* the bundle, which is what lets a bundle
/// mangle its whole tree while knowing nothing about its dependencies. Never
/// clone a mint — the copy would hand out indices that collide with the
/// original's while meaning something else.
#[derive(Debug)]
pub struct Mint {
    bundle: Bundle,
    names: IndexSet<String>,
    symbols: Vec<Data>,
    /// The one symbol at each global path. Not a symbol table for callers to
    /// resolve names against — deciding what a name means belongs to whoever
    /// owns the scopes. This is the guard that keeps two global symbols from
    /// sharing a path, and so keeps their manglings distinct.
    globals: HashMap<(Option<Module>, Namespace, Name), Symbol>,
    /// The next disambiguator for each local path.
    locals: HashMap<(Option<Module>, Namespace, Name), u32>,
}

/// Every bundle in one compilation, and the mint each one owns.
#[derive(Debug, Default)]
pub struct Bundles {
    entries: HashMap<BundleHash, Entry>,
}

#[derive(Debug)]
struct Entry {
    bundle: Bundle,
    /// `None` while the mint is checked out for minting.
    mint: Option<Mint>,
}

/// Why a bundle could not be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clash {
    /// The same name and version were registered twice.
    Duplicate,
    /// Two different bundles fingerprinted the same. Astronomically unlikely,
    /// and caught here rather than miscompiled.
    Fingerprint(Bundle),
}

/// One component of a demangled name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub namespace: Namespace,
    pub name: String,
    pub disambiguator: Option<u32>,
}

/// A mangled name taken apart again. The path runs outermost-first and ends
/// with the symbol itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Demangled {
    pub bundle: Bundle,
    pub path: Vec<Component>,
}

/// Renders a symbol as `bundle::module::name`. Kept separate from [`Symbol`]
/// because printing one needs the mint that made it.
struct DisplayPath<'a> {
    mint: &'a Mint,
    symbol: Symbol,
}

/// The canonical spelling of a bundle's identity. Bundle names cannot contain
/// `@`, so this is one string per bundle and one bundle per string — which is
/// what makes it safe to fingerprint.
impl fmt::Display for Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Namespace::Terms => f.write_str("term"),
            Namespace::Types => f.write_str("type"),
            Namespace::Modules => f.write_str("module"),
        }
    }
}

impl fmt::Display for DisplayPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The version is left out: this is for diagnostics, where it would be
        // noise everywhere except the rare cross-version confusion.
        f.write_str(&self.mint.bundle.name)?;
        for symbol in self.mint.chain(self.symbol) {
            write!(f, "::{}", self.mint.name(symbol))?;
        }
        Ok(())
    }
}

impl Bundle {
    /// `name` must be ASCII: a letter followed by letters, digits, `_`, or `-`.
    ///
    /// Build metadata is rejected rather than ignored. SemVer gives it no part
    /// in precedence and the mangling does not encode it, so `1.0.0+a` and
    /// `1.0.0+b` — two values that are `!=` to each other — would otherwise be
    /// two distinct bundles sharing one mangled name.
    pub fn new(name: &str, version: Version) -> Option<Self> {
        if !valid_name(name) || !version.build.is_empty() {
            return None;
        }
        let hash = BundleHash::of(&format!("{name}@{version}"));
        Some(Self {
            name: name.to_owned(),
            version,
            hash,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub const fn hash(&self) -> BundleHash {
        self.hash
    }
}

impl BundleHash {
    /// XXH3-64, whose output is pinned by its specification rather than by
    /// whatever the standard library happens to do this release; the answer has
    /// to be the same in every run and every tool.
    fn of(canonical: &str) -> Self {
        Self(XxHash3_64::oneshot(canonical.as_bytes()))
    }

    /// The fingerprint itself. Readable but not constructible: a hash only
    /// means anything if it came from a bundle's identity.
    pub const fn bits(self) -> u64 {
        self.0
    }
}

impl Symbol {
    pub const fn bundle(self) -> BundleHash {
        self.bundle
    }
}

impl Module {
    pub const fn symbol(self) -> Symbol {
        self.0
    }
}

impl From<Module> for Symbol {
    fn from(module: Module) -> Self {
        module.0
    }
}

impl Namespace {
    const fn tag(self) -> char {
        match self {
            Namespace::Terms => 'V',
            Namespace::Types => 'T',
            Namespace::Modules => 'M',
        }
    }

    const fn from_tag(tag: char) -> Option<Self> {
        match tag {
            'V' => Some(Namespace::Terms),
            'T' => Some(Namespace::Types),
            'M' => Some(Namespace::Modules),
            _ => None,
        }
    }
}

impl Mint {
    pub fn new(bundle: Bundle) -> Self {
        Self {
            bundle,
            names: IndexSet::new(),
            symbols: Vec::new(),
            globals: HashMap::new(),
            locals: HashMap::new(),
        }
    }

    pub fn bundle(&self) -> &Bundle {
        &self.bundle
    }

    /// Mint the symbol at a global path, or hand back the one already there.
    /// The `Err` carries the earlier symbol so a redeclaration can be reported
    /// against the declaration it collides with.
    pub fn global(
        &mut self,
        parent: Option<Module>,
        namespace: Namespace,
        name: &str,
    ) -> Result<Symbol, Symbol> {
        self.check_parent(parent);
        let name = self.intern(name);
        let key = (parent, namespace, name);
        if let Some(&existing) = self.globals.get(&key) {
            return Err(existing);
        }
        let symbol = self.push(Data {
            name,
            namespace,
            parent,
            disambiguator: None,
        });
        self.globals.insert(key, symbol);
        Ok(symbol)
    }

    /// [`global`](Self::global) in the module namespace.
    pub fn module(&mut self, parent: Option<Module>, name: &str) -> Result<Module, Module> {
        match self.global(parent, Namespace::Modules, name) {
            Ok(symbol) => Ok(Module(symbol)),
            Err(existing) => Err(Module(existing)),
        }
    }

    /// Mint a fresh local symbol. Two calls with identical arguments return
    /// unequal symbols; that is the whole point of a local.
    pub fn local(&mut self, parent: Option<Module>, namespace: Namespace, name: &str) -> Symbol {
        self.check_parent(parent);
        let name = self.intern(name);
        // Counted per path rather than per mint, so that adding a local in one
        // module does not renumber every local after it in the bundle — and so
        // that a mangling never depends on the order symbols were minted in.
        let disambiguator = {
            let next = self.locals.entry((parent, namespace, name)).or_insert(0);
            let current = *next;
            *next += 1;
            current
        };
        self.push(Data {
            name,
            namespace,
            parent,
            disambiguator: Some(disambiguator),
        })
    }

    /// The name the symbol was minted from, as written in the source.
    pub fn name(&self, symbol: Symbol) -> &str {
        let name = self.data(symbol).name;
        self.names
            .get_index(name.0 as usize)
            .expect("interned name was dropped")
    }

    pub fn namespace(&self, symbol: Symbol) -> Namespace {
        self.data(symbol).namespace
    }

    /// The containing module, or `None` for a symbol at the top level of the
    /// bundle.
    pub fn parent(&self, symbol: Symbol) -> Option<Module> {
        self.data(symbol).parent
    }

    pub fn is_local(&self, symbol: Symbol) -> bool {
        self.data(symbol).disambiguator.is_some()
    }

    pub fn is_global(&self, symbol: Symbol) -> bool {
        !self.is_local(symbol)
    }

    /// Every symbol this mint has made, in the order they were made.
    pub fn symbols(&self) -> impl Iterator<Item = Symbol> + '_ {
        (0..self.symbols.len() as u32).map(|index| Symbol {
            bundle: self.bundle.hash,
            index,
        })
    }

    /// `bundle::module::name`, for diagnostics. Not unique: two locals can
    /// share a path, which is exactly what [`mangle`](Self::mangle) is for.
    pub fn path(&self, symbol: Symbol) -> impl fmt::Display + '_ {
        DisplayPath { mint: self, symbol }
    }

    /// The symbol's strictly ASCII name, unique against every other symbol in
    /// every bundle. See [`demangle`] for the grammar.
    pub fn mangle(&self, symbol: Symbol) -> String {
        let mut out = String::from(PREFIX);
        write_bundle(&mut out, &self.bundle);
        for symbol in self.chain(symbol) {
            let data = self.data(symbol);
            write_component(
                &mut out,
                data.namespace.tag(),
                self.name(symbol),
                data.disambiguator,
            );
        }
        out
    }

    fn intern(&mut self, name: &str) -> Name {
        if let Some(index) = self.names.get_index_of(name) {
            return Name(index as u32);
        }
        let (index, _) = self.names.insert_full(name.to_owned());
        Name(index as u32)
    }

    fn push(&mut self, data: Data) -> Symbol {
        let index = self.symbols.len() as u32;
        self.symbols.push(data);
        Symbol {
            bundle: self.bundle.hash,
            index,
        }
    }

    fn data(&self, symbol: Symbol) -> &Data {
        debug_assert_eq!(
            symbol.bundle, self.bundle.hash,
            "symbol belongs to another bundle"
        );
        &self.symbols[symbol.index as usize]
    }

    fn check_parent(&self, parent: Option<Module>) {
        if let Some(parent) = parent {
            self.data(parent.0);
        }
    }

    /// The symbols from the outermost containing module down to `symbol`.
    fn chain(&self, symbol: Symbol) -> Vec<Symbol> {
        let mut chain = vec![symbol];
        let mut parent = self.data(symbol).parent;
        while let Some(module) = parent {
            chain.push(module.0);
            parent = self.data(module.0).parent;
        }
        chain.reverse();
        chain
    }
}

impl Bundles {
    /// Registering is where bundle identities are checked against each other,
    /// which is the one thing no single mint can do for itself.
    pub fn register(&mut self, bundle: Bundle) -> Result<(), Clash> {
        match self.entries.get(&bundle.hash) {
            Some(entry) if entry.bundle == bundle => Err(Clash::Duplicate),
            Some(entry) => Err(Clash::Fingerprint(entry.bundle.clone())),
            None => {
                self.entries.insert(
                    bundle.hash,
                    Entry {
                        mint: Some(Mint::new(bundle.clone())),
                        bundle,
                    },
                );
                Ok(())
            }
        }
    }

    pub fn contains(&self, hash: BundleHash) -> bool {
        self.entries.contains_key(&hash)
    }

    pub fn get(&self, hash: BundleHash) -> &Mint {
        self.entry(hash)
            .mint
            .as_ref()
            .expect("bundle is checked out")
    }

    /// Take a bundle's mint out so it can be minted into while every other
    /// bundle stays readable. The hole is only observable while that bundle is
    /// lowering itself, when nothing should be asking the registry about it —
    /// and if something does, it panics rather than quietly misbehaving.
    pub fn checkout(&mut self, hash: BundleHash) -> Mint {
        self.entries
            .get_mut(&hash)
            .expect("bundle is not registered")
            .mint
            .take()
            .expect("bundle is already checked out")
    }

    pub fn restore(&mut self, mint: Mint) {
        let entry = self
            .entries
            .get_mut(&mint.bundle.hash)
            .expect("bundle is not registered");
        debug_assert!(entry.mint.is_none(), "bundle was never checked out");
        entry.mint = Some(mint);
    }

    /// Route on the symbol's own bundle, so a caller holding symbols from
    /// several bundles never has to track which mint each one came from.
    pub fn name(&self, symbol: Symbol) -> &str {
        self.get(symbol.bundle).name(symbol)
    }

    pub fn mangle(&self, symbol: Symbol) -> String {
        self.get(symbol.bundle).mangle(symbol)
    }

    fn entry(&self, hash: BundleHash) -> &Entry {
        self.entries.get(&hash).expect("bundle is not registered")
    }
}

/// Take a mangled name apart. Being able to do this at all is what makes
/// [`Mint::mangle`] injective: distinct symbols differ in some component, and
/// no two component lists share a mangling.
///
/// ```text
/// mangled   := "_R" bundle component+
/// bundle    := "B" len body version
/// version   := "v" num "m" num "p" num [ "r" len body ]
/// component := ns len body [ "s" num ]         // ns = M | T | V
/// len       := the byte length of body, in decimal
/// body      := the name, with every character outside [A-Za-z0-9] — and a
///              leading digit — written as "_" hex "_"
/// ```
///
/// The length prefixes make component boundaries decidable without a separator,
/// bodies never start with a digit so `len` reads greedily, and neither `s` nor
/// a version tag is a namespace tag, so nothing structural can be read as the
/// next component. Only canonical manglings are accepted: a redundant escape or
/// a padded number is rejected rather than accepted alongside the real form.
pub fn demangle(mangled: &str) -> Option<Demangled> {
    let mut parser = Parser {
        rest: mangled.strip_prefix(PREFIX)?,
    };

    if !parser.eat(BUNDLE_TAG) {
        return None;
    }
    let name = parser.body()?;
    let version = parser.version()?;
    // Rebuilding through the constructor re-checks the identity rules, so a
    // mangling carrying a name or prerelease we would never emit is rejected.
    let bundle = Bundle::new(&name, version)?;

    let mut path = Vec::new();
    while !parser.rest.is_empty() {
        let namespace = Namespace::from_tag(parser.tag()?)?;
        let name = parser.body()?;
        let disambiguator = if parser.eat('s') {
            Some(parser.number()?)
        } else {
            None
        };
        path.push(Component {
            namespace,
            name,
            disambiguator,
        });
    }
    // A bundle on its own names no symbol.
    if path.is_empty() {
        return None;
    }
    Some(Demangled { bundle, path })
}

struct Parser<'a> {
    rest: &'a str,
}

impl Parser<'_> {
    fn eat(&mut self, c: char) -> bool {
        match self.rest.strip_prefix(c) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn tag(&mut self) -> Option<char> {
        let mut chars = self.rest.chars();
        let tag = chars.next()?;
        self.rest = chars.as_str();
        Some(tag)
    }

    /// A decimal number with no padding, so that each number has one spelling.
    fn number<T: std::str::FromStr>(&mut self) -> Option<T> {
        let end = self
            .rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(self.rest.len());
        let digits = &self.rest[..end];
        if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
            return None;
        }
        self.rest = &self.rest[end..];
        digits.parse().ok()
    }

    fn body(&mut self) -> Option<String> {
        let len: usize = self.number()?;
        let (body, rest) = self.rest.split_at_checked(len)?;
        self.rest = rest;
        unescape(body)
    }

    fn version(&mut self) -> Option<Version> {
        if !self.eat(MAJOR_TAG) {
            return None;
        }
        let major = self.number()?;
        if !self.eat(MINOR_TAG) {
            return None;
        }
        let minor = self.number()?;
        if !self.eat(PATCH_TAG) {
            return None;
        }
        let patch = self.number()?;

        let mut version = Version::new(major, minor, patch);
        if self.eat(PRERELEASE_TAG) {
            let pre = Prerelease::new(&self.body()?).ok()?;
            // The tag is only ever written for a non-empty prerelease, so `r0`
            // is not something we would have produced.
            if pre.is_empty() {
                return None;
            }
            version.pre = pre;
        }
        Some(version)
    }
}

/// Whether a character has to be escaped. A digit is only a problem at the
/// front of a body, where it would be eaten by the length prefix.
fn needs_escape(c: char, first: bool) -> bool {
    !c.is_ascii_alphanumeric() || (first && c.is_ascii_digit())
}

/// Rewrite a name as ASCII. An escape is `_`, the codepoint in uppercase hex,
/// `_` — so every `_` in the result either opens or closes one, and an escape
/// is never empty. That is what makes [`unescape`] unambiguous.
fn escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, c) in name.chars().enumerate() {
        if needs_escape(c, i == 0) {
            // Writing to a `String` cannot fail.
            let _ = write!(out, "_{:X}_", c as u32);
        } else {
            out.push(c);
        }
    }
    out
}

fn unescape(body: &str) -> Option<String> {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        // Exactly one character is pushed per iteration, so this is the
        // position the escaping rules were applied at.
        let first = out.is_empty();
        if c != '_' {
            if needs_escape(c, first) {
                return None;
            }
            out.push(c);
            continue;
        }
        let mut hex = String::new();
        loop {
            match chars.next()? {
                '_' => break,
                digit @ ('0'..='9' | 'A'..='F') => hex.push(digit),
                _ => return None,
            }
        }
        if hex.is_empty() || (hex.len() > 1 && hex.starts_with('0')) {
            return None;
        }
        let decoded = char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?;
        // An escape for a character that did not need one is not canonical.
        if !needs_escape(decoded, first) {
            return None;
        }
        out.push(decoded);
    }
    Some(out)
}

fn write_bundle(out: &mut String, bundle: &Bundle) {
    let version = &bundle.version;
    write_component(out, BUNDLE_TAG, &bundle.name, None);
    // Version parts are digits, so they need neither escaping nor a length.
    let _ = write!(
        out,
        "{MAJOR_TAG}{}{MINOR_TAG}{}{PATCH_TAG}{}",
        version.major, version.minor, version.patch
    );
    if !version.pre.is_empty() {
        write_component(out, PRERELEASE_TAG, version.pre.as_str(), None);
    }
}

fn write_component(out: &mut String, tag: char, name: &str, disambiguator: Option<u32>) {
    let body = escape(name);
    out.push(tag);
    let _ = write!(out, "{}", body.len());
    out.push_str(&body);
    if let Some(disambiguator) = disambiguator {
        let _ = write!(out, "s{disambiguator}");
    }
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
