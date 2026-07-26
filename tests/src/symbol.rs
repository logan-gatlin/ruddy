//! Tests for [`ruddy::symbol`].

use std::collections::HashSet;

use ruddy::symbol::{
    Bundle, Bundles, Clash, Component, Mint, Namespace, PREFIX, Version, demangle,
};

/// Every mangling in these tests starts with this, for `app@1.0.0`.
const APP_PREFIX: &str = "_RB3appv1m0p0";

fn bundle(name: &str) -> Bundle {
    Bundle::new(name, Version::new(1, 0, 0)).expect("valid bundle")
}

fn mint() -> Mint {
    Mint::new(bundle("app"))
}

#[test]
fn bundle_identity_is_a_value() {
    // Same name and version, built independently: the same bundle.
    assert_eq!(bundle("app"), bundle("app"));
    assert_eq!(bundle("app").hash(), bundle("app").hash());

    let other = Bundle::new("app", Version::new(1, 0, 1)).unwrap();
    assert_ne!(bundle("app"), other);
    assert_ne!(bundle("app").hash(), other.hash());
    assert_eq!(bundle("app").to_string(), "app@1.0.0");
}

#[test]
fn the_fingerprint_is_pinned() {
    // A fingerprint is only worth anything if every build of every tool
    // agrees on it, so a hash that quietly changed under us is a break.
    assert_eq!(bundle("app").hash().bits(), 0x574e_1a3d_35e8_cad8);
}

#[test]
fn rejects_names_it_could_not_round_trip() {
    assert!(Bundle::new("", Version::new(1, 0, 0)).is_none());
    assert!(Bundle::new("1app", Version::new(1, 0, 0)).is_none());
    assert!(Bundle::new("my app", Version::new(1, 0, 0)).is_none());
    assert!(Bundle::new("app@1", Version::new(1, 0, 0)).is_none());
    assert!(Bundle::new("héllo", Version::new(1, 0, 0)).is_none());
    assert!(Bundle::new("my-app_2", Version::new(1, 0, 0)).is_some());
}

#[test]
fn rejects_build_metadata() {
    // `1.0.0+a` and `1.0.0+b` are distinct versions that would mangle
    // identically, so they cannot be allowed to name a bundle at all.
    let built = Version::parse("1.0.0+build.7").unwrap();
    assert_ne!(built, Version::new(1, 0, 0));
    assert!(Bundle::new("app", built).is_none());
    assert!(Bundle::new("app", Version::parse("1.0.0-alpha.1").unwrap()).is_some());
}

#[test]
fn locals_with_the_same_name_are_distinct() {
    let mut m = mint();
    let first = m.local(None, Namespace::Terms, "x");
    let second = m.local(None, Namespace::Terms, "x");

    assert_ne!(first, second);
    assert_ne!(m.mangle(first), m.mangle(second));
    // Otherwise identical: same name, namespace, and containing module.
    assert_eq!(m.name(first), m.name(second));
    assert_eq!(m.parent(first), m.parent(second));
    assert!(m.is_local(first) && m.is_local(second));
}

#[test]
fn globals_are_unique_and_report_the_first() {
    let mut m = mint();
    let first = m.global(None, Namespace::Terms, "map").unwrap();

    // The redeclaration hands back the declaration it collides with.
    assert_eq!(m.global(None, Namespace::Terms, "map"), Err(first));
    assert!(m.is_global(first));
}

#[test]
fn namespaces_are_disjoint() {
    let mut m = mint();
    let term = m.global(None, Namespace::Terms, "Foo").unwrap();
    let ty = m.global(None, Namespace::Types, "Foo").unwrap();
    let module = m.module(None, "Foo").unwrap();

    assert_ne!(term, ty);
    assert_ne!(term, module.symbol());
    assert_ne!(m.mangle(term), m.mangle(ty));
    assert_ne!(m.mangle(term), m.mangle(module.symbol()));
    assert_eq!(m.namespace(module.symbol()), Namespace::Modules);
}

#[test]
fn the_bundle_top_level_is_a_scope_of_its_own() {
    let mut m = mint();
    let util = m.module(None, "util").unwrap();
    let top = m.global(None, Namespace::Terms, "x").unwrap();
    let nested = m.global(Some(util), Namespace::Terms, "x").unwrap();

    assert_ne!(top, nested);
    assert_eq!(m.parent(top), None);
    assert_eq!(m.parent(nested), Some(util));
    // Each is a repeat only of its own scope, so neither path is taken by
    // the other.
    assert_eq!(m.global(None, Namespace::Terms, "x"), Err(top));
    assert_eq!(m.global(Some(util), Namespace::Terms, "x"), Err(nested));
}

#[test]
fn locals_do_not_occupy_a_global_path() {
    let mut m = mint();
    let local = m.local(None, Namespace::Terms, "x");

    // A local never takes the path, so the global is still free.
    let global = m.global(None, Namespace::Terms, "x").unwrap();
    assert_ne!(local, global);
    assert_ne!(m.mangle(local), m.mangle(global));
}

#[test]
fn modules_nest() {
    let mut m = mint();
    let outer = m.module(None, "a").unwrap();
    let inner = m.module(Some(outer), "b").unwrap();
    let leaf = m.global(Some(inner), Namespace::Terms, "c").unwrap();

    assert_eq!(m.parent(leaf), Some(inner));
    assert_eq!(m.parent(inner.symbol()), Some(outer));
    assert_eq!(m.parent(outer.symbol()), None);
    assert_eq!(m.path(leaf).to_string(), "app::a::b::c");
    assert_eq!(m.mangle(leaf), format!("{APP_PREFIX}M1aM1bV1c"));
}

#[test]
fn mangles_the_documented_examples() {
    let mut m = mint();
    let util = m.module(None, "util").unwrap();
    let top_map = m.global(None, Namespace::Terms, "map").unwrap();
    let util_map = m.global(Some(util), Namespace::Terms, "map").unwrap();
    let util_ty = m.global(Some(util), Namespace::Types, "Map").unwrap();
    let top_x = m.local(None, Namespace::Terms, "x");
    let util_x = m.local(Some(util), Namespace::Terms, "x");
    let another_top_x = m.local(None, Namespace::Terms, "x");

    assert_eq!(m.mangle(util.symbol()), format!("{APP_PREFIX}M4util"));
    assert_eq!(m.mangle(top_map), format!("{APP_PREFIX}V3map"));
    assert_eq!(m.mangle(util_map), format!("{APP_PREFIX}M4utilV3map"));
    assert_eq!(m.mangle(util_ty), format!("{APP_PREFIX}M4utilT3Map"));
    assert_eq!(m.mangle(top_x), format!("{APP_PREFIX}V1xs0"));
    assert_eq!(m.mangle(util_x), format!("{APP_PREFIX}M4utilV1xs0"));
    // The counter is per path, so the second top-level `x` is `s1` even
    // though a local in `util` was minted between them.
    assert_eq!(m.mangle(another_top_x), format!("{APP_PREFIX}V1xs1"));
}

#[test]
fn the_version_is_part_of_the_mangling() {
    let version = Version::parse("10.2.0-alpha.1").unwrap();
    let mut m = Mint::new(Bundle::new("app", version).unwrap());
    let symbol = m.global(None, Namespace::Terms, "map").unwrap();

    assert_eq!(m.mangle(symbol), "_RB3appv10m2p0r10alpha_2E_1V3map");
    assert_eq!(
        demangle(&m.mangle(symbol)).unwrap().bundle,
        *m.bundle(),
        "the bundle did not survive the round trip"
    );
    assert_eq!(m.bundle().to_string(), "app@10.2.0-alpha.1");
}

#[test]
fn mangles_are_ascii_identifiers() {
    let mut m = Mint::new(bundle("my-app_2"));
    let module = m.module(None, "héllo").unwrap();
    let symbols = [
        m.global(Some(module), Namespace::Terms, "with_underscores")
            .unwrap(),
        m.global(Some(module), Namespace::Types, "Ünïcode").unwrap(),
        m.local(Some(module), Namespace::Terms, "<closure>"),
        m.global(None, Namespace::Terms, "0day").unwrap(),
    ];

    for symbol in symbols {
        let mangled = m.mangle(symbol);
        assert!(
            mangled.starts_with(PREFIX)
                && mangled
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "not an ASCII identifier: {mangled:?}"
        );
    }
}

#[test]
fn mangling_round_trips() {
    let mut m = Mint::new(bundle("my-app_2"));
    let module = m.module(None, "héllo").unwrap();
    let cases = [
        ("with_underscores", Namespace::Terms, false),
        ("Ünïcode", Namespace::Types, false),
        ("<closure>", Namespace::Terms, true),
        ("0day", Namespace::Terms, false),
        ("_", Namespace::Terms, true),
        ("", Namespace::Terms, false),
    ];

    for (name, namespace, local) in cases {
        let symbol = if local {
            m.local(Some(module), namespace, name)
        } else {
            m.global(Some(module), namespace, name).unwrap()
        };
        let mangled = m.mangle(symbol);
        let demangled =
            demangle(&mangled).unwrap_or_else(|| panic!("could not demangle {mangled:?}"));

        assert_eq!(demangled.bundle, *m.bundle());
        assert_eq!(
            demangled.path,
            vec![
                Component {
                    namespace: Namespace::Modules,
                    name: "héllo".to_owned(),
                    disambiguator: None,
                },
                Component {
                    namespace,
                    name: name.to_owned(),
                    disambiguator: local.then_some(0),
                },
            ],
            "round trip failed for {mangled:?}"
        );
    }
}

#[test]
fn round_trips_a_symbol_with_no_containing_module() {
    let mut m = mint();
    let symbol = m.global(None, Namespace::Terms, "map").unwrap();
    let demangled = demangle(&m.mangle(symbol)).unwrap();

    assert_eq!(demangled.bundle, *m.bundle());
    assert_eq!(demangled.path.len(), 1);
    assert_eq!(demangled.path[0].name, "map");
}

#[test]
fn mangles_are_unique_within_a_mint() {
    let mut m = mint();
    let util = m.module(None, "util").unwrap();
    let nested = m.module(Some(util), "util").unwrap();
    // `xs0` beside a local `x` is the near-collision the length prefix
    // exists to rule out: `V3xs0` against `V1xs0`.
    m.global(None, Namespace::Terms, "xs0").unwrap();
    m.global(None, Namespace::Terms, "x").unwrap();
    m.global(None, Namespace::Types, "x").unwrap();
    m.global(Some(util), Namespace::Terms, "x").unwrap();
    m.global(Some(nested), Namespace::Terms, "x").unwrap();
    m.local(None, Namespace::Terms, "x");
    m.local(None, Namespace::Terms, "x");
    m.local(None, Namespace::Types, "x");
    m.local(Some(util), Namespace::Terms, "x");

    let symbols: Vec<_> = m.symbols().collect();
    let mangled: HashSet<_> = symbols.iter().map(|&s| m.mangle(s)).collect();
    assert_eq!(mangled.len(), symbols.len(), "mangled: {mangled:#?}");
}

#[test]
fn bundles_mangle_independently() {
    // Same name, different version: still two bundles.
    let mut app = Mint::new(bundle("app"));
    let mut newer = Mint::new(Bundle::new("app", Version::new(2, 0, 0)).unwrap());
    let mut other = Mint::new(bundle("std"));

    let mut mangled = HashSet::new();
    for m in [&mut app, &mut newer, &mut other] {
        let util = m.module(None, "util").unwrap();
        m.global(Some(util), Namespace::Terms, "map").unwrap();
        m.local(Some(util), Namespace::Terms, "x");
    }
    for m in [&app, &newer, &other] {
        let symbols: Vec<_> = m.symbols().collect();
        assert_eq!(symbols.len(), 3);
        mangled.extend(symbols.into_iter().map(|s| m.mangle(s)));
    }

    // Identical trees, and still no shared name.
    assert_eq!(mangled.len(), 9);
}

#[test]
fn demangle_rejects_malformed_and_non_canonical_names() {
    let whole = [
        "B3appv1m0p0V1x",                 // no prefix
        "_RM3appv1m0p0V1x",               // no bundle tag
        "_RB3appV1x",                     // no version
        "_RB3appv1m0V1x",                 // version cut short
        "_RB3appv01m0p0V1x",              // padded major
        "_RB3appv1m0p0",                  // a bundle alone names no symbol
        "_RB7_30_appv1m0p0V1x",           // a name the identity rules forbid
        "_RB3appv1m0p0r11alpha_2E_01V1x", // a prerelease SemVer forbids
        "_RB3appv1m0p0r0V1x",             // an empty prerelease
    ];
    let components = [
        "X1x",     // unknown namespace tag
        "V2x",     // length runs past the end
        "V1x!",    // trailing garbage
        "V1xs",    // disambiguator with no number
        "V01x",    // padded length
        "V1xs00",  // padded disambiguator
        "V4_41_",  // escape for a character that needs none
        "V5_05F_", // padded hex
        "V2__",    // empty escape
        "V3_5F",   // unterminated escape
        "V4_5f_",  // lowercase hex
    ];

    for case in whole {
        assert!(demangle(case).is_none(), "accepted {case:?}");
    }
    for case in components {
        let case = format!("{APP_PREFIX}{case}");
        assert!(demangle(&case).is_none(), "accepted {case:?}");
    }
}

#[test]
fn a_leading_digit_is_escaped_only_at_the_front() {
    let mut m = mint();
    let symbol = m.global(None, Namespace::Terms, "0a0").unwrap();

    assert_eq!(m.mangle(symbol), format!("{APP_PREFIX}V6_30_a0"));
    assert_eq!(demangle(&m.mangle(symbol)).unwrap().path[0].name, "0a0");
}

#[test]
fn the_registry_owns_one_mint_per_bundle() {
    let mut bundles = Bundles::default();
    let app = bundle("app");
    let std = bundle("std");
    bundles.register(app.clone()).unwrap();
    bundles.register(std.clone()).unwrap();

    assert_eq!(bundles.register(app.clone()), Err(Clash::Duplicate));
    assert!(bundles.contains(app.hash()));

    // Check a mint out to write to it, while the others stay readable.
    let mut mint = bundles.checkout(app.hash());
    let symbol = mint.global(None, Namespace::Terms, "map").unwrap();
    assert!(bundles.contains(std.hash()));
    bundles.restore(mint);

    // Reading a symbol needs no idea which bundle it came from.
    assert_eq!(bundles.name(symbol), "map");
    assert_eq!(bundles.mangle(symbol), format!("{APP_PREFIX}V3map"));
}

#[test]
#[should_panic(expected = "checked out")]
fn a_checked_out_bundle_cannot_be_read() {
    let mut bundles = Bundles::default();
    let app = bundle("app");
    bundles.register(app.clone()).unwrap();

    let _mint = bundles.checkout(app.hash());
    let _ = bundles.get(app.hash());
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "another bundle")]
fn symbols_do_not_cross_bundles() {
    let mut app = mint();
    let other = Mint::new(bundle("std"));
    let symbol = app.global(None, Namespace::Terms, "x").unwrap();

    let _ = other.name(symbol);
}
