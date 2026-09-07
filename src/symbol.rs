use std::{collections::HashMap, fmt::Write as _};

use indexmap::{IndexMap, IndexSet};
use semver::Prerelease;
use twox_hash::XxHash3_64;

pub use semver::Version;

use crate::ui::Path;

/// Marks a string as one of our mangled names. Also guarantees the mangling
/// starts with a character that is legal at the front of an identifier.
pub const PREFIX: &str = "_R";

/// The path segment standing in for the scope a local lives in.
///
/// A local has no canonical path — the lambda or `let` body it belongs to is
/// not a module and has no name to spell — so every local of a given owner is
/// shown under one anonymous segment inside it, rather than directly beside
/// the globals it is not addressable alongside. Display only: the mangling
/// distinguishes locals by disambiguator, and never emits a component for this.
pub const LOCAL_SEGMENT: &str = "_";

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
/// A symbol is a value of its path, not a ticket from a counter: the fingerprint
/// of the mangled components that name it, so the same declaration is the same
/// symbol in every run and whatever else was minted before it. Adding a local
/// to one definition renumbers nothing in another, which is what lets the
/// lowered form of a definition nobody edited compare equal to what it was.
/// Everything else about it — its name, namespace, containing module and owner
/// — is held by the [`Mint`] that made it. Symbols carry the bundle they were
/// minted in so that two bundles' symbols can never compare equal by accident.
///
/// Ordered, so that it can key an ordered collection, but the order is the
/// fingerprint's and means nothing; what wants source order asks the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, salsa::SalsaValue)]
pub struct Symbol {
    bundle: BundleHash,
    path: u64,
}

/// A symbol already known to live in the module namespace. Newtyped so that a
/// term can never be passed where a containing module is expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Module(Symbol);

/// The four disjoint worlds a symbol can live in. The same name in two
/// namespaces makes two unrelated symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    Terms,
    Types,
    /// Declared effects. Their own world rather than a corner of the types',
    /// because a `Log` type and a `Log` effect are two things a program may
    /// name at once and nothing written can confuse them: an effect is named in
    /// a row after a `+` and at the head of an operation, and a type nowhere
    /// near either.
    Effects,
    Modules,
}

/// Index into the mint's interned name table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Name(u32);

/// Everything the mint knows about one symbol. Spans are deliberately absent;
/// callers keep a [`Tracked<Symbol>`](crate::tracking::Tracked) instead, since
/// one symbol can be written at many places.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Data {
    name: Name,
    namespace: Namespace,
    /// `None` places the symbol directly at the top level of the bundle, which
    /// is a real position in the tree rather than a missing one.
    parent: Option<Module>,
    /// The definition a local was written inside, when it was written inside
    /// one. A local's path runs through its owner, so that its number counts
    /// only the locals of that definition.
    owner: Option<Symbol>,
    /// Present when the symbol is local.
    disambiguator: Option<u32>,
}

/// Builder for unique symbols.
///
/// One mint per bundle: the mint *is* the bundle, which is what lets a bundle
/// mangle its whole tree while knowing nothing about its dependencies. Symbols
/// are fingerprints of their paths, so two mints of one bundle minting the same
/// declarations hand out the same symbols; what a mint holds beyond that is
/// the names behind them and the count of locals at each path.
#[derive(Debug, Clone)]
pub struct Mint {
    bundle: Bundle,
    names: IndexSet<String>,
    /// Every symbol minted, in the order it was minted, and what it names.
    symbols: IndexMap<Symbol, Data>,
    /// The next disambiguator for each local path: the owner, or the module
    /// for a local that no definition wrote, then the namespace and name.
    locals: HashMap<(Option<Symbol>, Option<Module>, Namespace, Name), u32>,
    /// Portable artifact spelling for dependency-interface symbols represented
    /// inside this mint. These symbols participate in checking but retain the
    /// bundle that actually owns them at the artifact boundary.
    external: HashMap<Symbol, String>,
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
        // The same string [`Display for Bundle`](crate::ui) writes, and
        // deliberately not that impl: the fingerprint reaches every mangled
        // name in a build, so it must not follow a reworded display.
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
    /// The symbol an [`Anchor`](crate::tracking::Anchor) names when what it
    /// anchors was written by no definition: what the compiler generated, or
    /// a dependency's declaration. No mint ever hands this out, so nothing a
    /// program declares can collide with it.
    pub const GENERATED: Symbol = Symbol {
        bundle: BundleHash(0),
        path: u64::MAX,
    };

    pub const fn bundle(self) -> BundleHash {
        self.bundle
    }

    /// The fingerprint of the symbol's path within its bundle. Readable but
    /// not constructible, like [`BundleHash::bits`].
    pub const fn bits(self) -> u64 {
        self.path
    }
}

/// What a symbol is before anything is named: the generated one, so that a
/// record with a symbol in it can be built empty and filled in.
impl Default for Symbol {
    fn default() -> Self {
        Self::GENERATED
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
            Namespace::Effects => 'E',
            Namespace::Modules => 'M',
        }
    }

    const fn from_tag(tag: char) -> Option<Self> {
        match tag {
            'V' => Some(Namespace::Terms),
            'T' => Some(Namespace::Types),
            'E' => Some(Namespace::Effects),
            'M' => Some(Namespace::Modules),
            _ => None,
        }
    }
}

impl Mint {
    /// A read-only name snapshot for one query. Keep owner/module paths, but
    /// avoid retaining the complete source mint for every edited definition.
    pub(crate) fn project(&self, symbols: impl IntoIterator<Item = Symbol>) -> Self {
        let mut projected = Self::new(self.bundle.clone());
        let mut work: Vec<_> = symbols.into_iter().collect();
        while let Some(symbol) = work.pop() {
            if projected.symbols.contains_key(&symbol) {
                continue;
            }
            let Some(data) = self.symbols.get(&symbol) else {
                continue;
            };
            let mut data = data.clone();
            data.name = Name(projected.names.insert_full(self.name(symbol).to_owned()).0 as u32);
            work.extend(data.owner);
            work.extend(data.parent.map(Module::symbol));
            projected.symbols.insert(symbol, data);
            if let Some(name) = self.external.get(&symbol) {
                projected.external.insert(symbol, name.clone());
            }
        }
        projected
    }

    pub fn new(bundle: Bundle) -> Self {
        Self {
            bundle,
            names: IndexSet::new(),
            symbols: IndexMap::new(),
            locals: HashMap::new(),
            external: HashMap::new(),
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
        self.push(Data {
            name,
            namespace,
            parent,
            owner: None,
            disambiguator: None,
        })
    }

    /// [`global`](Self::global) in the module namespace.
    pub fn module(&mut self, parent: Option<Module>, name: &str) -> Result<Module, Module> {
        match self.global(parent, Namespace::Modules, name) {
            Ok(symbol) => Ok(Module(symbol)),
            Err(existing) => Err(Module(existing)),
        }
    }

    /// Mint a fresh local symbol that no definition wrote: an imported
    /// declaration's stand-in, or a test's. Two calls with identical arguments
    /// return unequal symbols; that is the whole point of a local.
    ///
    /// A local written inside a definition wants [`local_in`](Self::local_in),
    /// which counts it against that definition alone.
    pub fn local(&mut self, parent: Option<Module>, namespace: Namespace, name: &str) -> Symbol {
        self.check_parent(parent);
        self.fresh(parent, None, namespace, name)
    }

    /// Mint a fresh local symbol of the definition `owner`, in the module the
    /// owner is in. Counted per owner, name and namespace, so that a local
    /// added to one definition renumbers no local of another, and its symbol
    /// — a fingerprint of its path — says nothing about what else was minted.
    pub fn local_in(&mut self, owner: Symbol, namespace: Namespace, name: &str) -> Symbol {
        let parent = self.data(owner).parent;
        self.fresh(parent, Some(owner), namespace, name)
    }

    fn fresh(
        &mut self,
        parent: Option<Module>,
        owner: Option<Symbol>,
        namespace: Namespace,
        name: &str,
    ) -> Symbol {
        let name = self.intern(name);
        let disambiguator = {
            let next = self
                .locals
                .entry((owner, parent, namespace, name))
                .or_insert(0);
            let current = *next;
            *next += 1;
            current
        };
        self.push(Data {
            name,
            namespace,
            parent,
            owner,
            disambiguator: Some(disambiguator),
        })
        .unwrap_or_else(|_| unreachable!("a local's disambiguator is fresh"))
    }

    /// Associate an imported semantic symbol with its portable qualified name.
    pub fn register_external(&mut self, symbol: Symbol, qualified: impl Into<String>) {
        self.data(symbol);
        self.external.insert(symbol, qualified.into());
    }

    /// The portable qualified name of an imported symbol.
    pub fn external(&self, symbol: Symbol) -> Option<&str> {
        self.external.get(&symbol).map(String::as_str)
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

    /// The definition a local was written inside, or `None` for a global and
    /// for a local no definition wrote.
    pub fn owner(&self, symbol: Symbol) -> Option<Symbol> {
        self.data(symbol).owner
    }

    pub fn is_local(&self, symbol: Symbol) -> bool {
        self.data(symbol).disambiguator.is_some()
    }

    pub fn is_global(&self, symbol: Symbol) -> bool {
        !self.is_local(symbol)
    }

    /// Every symbol this mint has made, in the order they were made.
    pub fn symbols(&self) -> impl Iterator<Item = Symbol> + '_ {
        self.symbols.keys().copied()
    }

    /// `bundle::module::name`, for diagnostics, with every local one segment
    /// further in under [`LOCAL_SEGMENT`]: `demo::_::x` for the `x` bound by
    /// `fn x => ...` at the top level of `demo`.
    ///
    /// Not unique, and not meant to be: two locals of the same parent share a
    /// path, and a global genuinely named `_` reads like the marker. Telling
    /// those apart is what [`mangle`](Self::mangle) is for.
    pub fn path(&self, symbol: Symbol) -> Path<'_> {
        Path::new(self, symbol)
    }

    /// The symbol's strictly ASCII name, unique against every other symbol in
    /// every bundle. See [`demangle`] for the grammar.
    pub fn mangle(&self, symbol: Symbol) -> String {
        let mut out = String::from(PREFIX);
        write_bundle(&mut out, &self.bundle);
        self.write_path(&mut out, self.data(symbol));
        out
    }

    /// The mangled components of `data`'s path, outermost first, with no
    /// bundle in front: what a symbol is the fingerprint of.
    fn write_path(&self, out: &mut String, data: &Data) {
        for symbol in self.above(data) {
            self.write_data(out, self.data(symbol));
        }
        self.write_data(out, data);
    }

    fn write_data(&self, out: &mut String, data: &Data) {
        let name = self
            .names
            .get_index(data.name.0 as usize)
            .expect("interned name was dropped");
        write_component(out, data.namespace.tag(), name, data.disambiguator);
    }

    fn intern(&mut self, name: &str) -> Name {
        if let Some(index) = self.names.get_index_of(name) {
            return Name(index as u32);
        }
        let (index, _) = self.names.insert_full(name.to_owned());
        Name(index as u32)
    }

    /// The symbol at `data`'s path, minted if it was not already. The `Err`
    /// carries the symbol already there, which is the same declaration named
    /// twice: a redeclaration for a global, and impossible for a local, whose
    /// disambiguator is fresh.
    ///
    /// The path is sixty-four bits of fingerprint, so two paths that agree on
    /// it would be miscompiled as one symbol. That is ruled out here rather
    /// than trusted: a program cannot be written to produce it, and a check
    /// nothing can reach is one nobody can confirm still means what it says.
    fn push(&mut self, data: Data) -> Result<Symbol, Symbol> {
        let mut path = String::new();
        self.write_path(&mut path, &data);
        let symbol = Symbol {
            bundle: self.bundle.hash,
            path: XxHash3_64::oneshot(path.as_bytes()),
        };
        match self.symbols.get(&symbol) {
            Some(existing) if *existing == data => Err(symbol),
            Some(existing) => {
                let mut other = String::new();
                self.write_path(&mut other, existing);
                panic!("symbol paths {path} and {other} fingerprint the same")
            }
            None => {
                self.symbols.insert(symbol, data);
                Ok(symbol)
            }
        }
    }

    fn data(&self, symbol: Symbol) -> &Data {
        debug_assert_eq!(
            symbol.bundle, self.bundle.hash,
            "symbol belongs to another bundle"
        );
        self.symbols
            .get(&symbol)
            .expect("symbol was minted by another mint")
    }

    fn check_parent(&self, parent: Option<Module>) {
        if let Some(parent) = parent {
            self.data(parent.0);
        }
    }

    /// The symbols from the outermost containing module down to `symbol`,
    /// through the definition that owns it when one does. Crate-visible so
    /// that [`Path`] can walk it; a caller outside wants [`path`](Self::path)
    /// or [`mangle`](Self::mangle), which are the two things a chain is ever
    /// walked for.
    pub(crate) fn chain(&self, symbol: Symbol) -> Vec<Symbol> {
        let mut chain = self.above(self.data(symbol));
        chain.push(symbol);
        chain
    }

    /// Everything on the path above `data`, outermost first: its owner if it
    /// has one, and the modules from there out.
    fn above(&self, data: &Data) -> Vec<Symbol> {
        let mut above = Vec::new();
        let mut at = data.owner.or(data.parent.map(Module::symbol));
        while let Some(symbol) = at {
            above.push(symbol);
            let data = self.data(symbol);
            at = data.owner.or(data.parent.map(Module::symbol));
        }
        above.reverse();
        above
    }
}

impl Clash {
    /// What it means that `arriving` fingerprints the same as the `registered`
    /// bundle: either it is that bundle, or two identities have collided.
    ///
    /// A named rule rather than an arm inside [`Bundles::register`], because a
    /// collision is a thing no program can be made to produce — the fingerprint
    /// is sixty-four bits of the identity — and a check nothing can reach is a
    /// check nobody can confirm still says what it means to.
    pub fn between(registered: &Bundle, arriving: &Bundle) -> Self {
        match registered == arriving {
            true => Clash::Duplicate,
            false => Clash::Fingerprint(registered.clone()),
        }
    }
}

impl Bundles {
    /// Registering is where bundle identities are checked against each other,
    /// which is the one thing no single mint can do for itself.
    pub fn register(&mut self, bundle: Bundle) -> Result<(), Clash> {
        match self.entries.get(&bundle.hash) {
            Some(entry) => Err(Clash::between(&entry.bundle, &bundle)),
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
/// component := ns len body [ "s" num ]         // ns = E | M | T | V
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
        let namespace = Namespace::from_tag(parser.tag())?;
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

    /// The next character, which the caller has already found to be there.
    fn tag(&mut self) -> char {
        let mut chars = self.rest.chars();
        let tag = chars
            .next()
            .expect("the caller checked there is more to read");
        self.rest = chars.as_str();
        tag
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
