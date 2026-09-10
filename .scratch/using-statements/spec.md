# Lexical using statements

Confirmed by the user through the design interview and implementation request.

- `using path` binds the final name; `using path::*` imports accessible declarations of the target module, including declared submodules but excluding imported names.
- Support `as` aliases, nested brace groups, and grouped `self` (including `self as alias`).
- Module imports are hoisted throughout the containing module and nested scopes. Import chains can refer forward; unresolvable chains fail.
- Local imports are statements inside `do` blocks, sequential from the statement onward, and may reference only names already in scope. They do not escape the block.
- Imports participate in all matching namespaces and preserve existing bundle-private access rules.
- Explicit imports conflict with explicit imports or declarations in the same namespace and scope. Explicit names override globs. Distinct glob targets are ambiguous only when used; inner scopes shadow outer scopes, and imports override the prelude.
- Imported names create neither bundle exports nor qualified members of the importing module. Public declarations can use imported types/effects without exporting their aliases.
- `bundle`, `self`, and repeated `super` anchor import and ordinary value/type/effect paths. Above-root traversal fails; unprefixed paths retain existing lookup rules.
- Bare anchors require aliases. Grouped `self` binds the selected module.
- Validate every import even when unused. Defer unused-import warnings.

Review baseline: `74869e0b9c0f7e9edeb2cd6cc65b2c565d7cdaa5`.
Validation boundaries: existing compiler source-language and bundle artifact/export APIs, selected within the authorized implementation scope.
