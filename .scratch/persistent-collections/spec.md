# Persistent maps and sets

Implement the interfaces agreed in the conversation, as `std::map` and `std::set`.

- Persistent HAMT storage shared by both modules; updates share unchanged paths.
  Opaque storage, one fixed hash seed, cached full hashes, collision equality,
  and unspecified traversal order. No builders, custom hashers or mutable tables.
- Map: empty, len, is_empty, get, contains, insert, remove, from_array,
  to_array, keys, values, fold, map_values, equal_by.
- Set: empty, len, is_empty, contains, insert, remove, from_array, to_array,
  union, intersection, difference, is_subset, equal, fold.
- Ordinary lookups treat unhashable keys as absent; insert/remove leave the
  original unchanged. Forgiving from_array skips bad entries individually.
- Add try_get (map), try_contains, try_insert, try_remove, try_from_array.
  Return Result with hash::Error. Missing valid keys succeed. Strict constructors
  stop at the first bad entry, return no partial collection, and prepend its
  array index to the key/element's hashing error path.
- Map insertion replaces equal keys' values; the last accepted duplicate wins
  in construction. Set insertion is idempotent. Difference left right retains
  elements of left absent from right. Values need not be hashable.
- Fold and map_values propagate callback effects. Equality compares contents,
  independent of insertion history/trie layout; map equality accepts a value
  comparator. Operations on accepted entries reuse cached hashes.
- Test at the public map/set interfaces, covering persistence, collisions,
  forgiving and strict failures, bulk paths, traversal, transforms, algebra,
  value freedom, and interpreter/JavaScript agreement. Rust tests only via
  just test. Typecheck regularly and run the full suite at the end.
- Generate API documentation, run parallel standards/spec code reviews, and
  commit the finished work on the current branch.
