---
name: Hunkset query language
description: Design decisions for the hunkset algebraic query language for selecting hunks, modeled after jj filesets/revsets
type: project
---

Hunkset is an algebraic query language for selecting hunks, introduced as an alternative to JSON/YAML specs.

**Key design decisions:**
- No `index()` function — indices are unstable and defeat the purpose of a query language
- `id()` is the only direct-addressing mechanism (via stable SHA256 hunk IDs)
- `file("x") & type(insert)` uses intersection naturally since hunks belong to exactly one file
- Tree-sitter is planned for `function()` and `scope()` semantic predicates — not feature-gated, standard dependency
- Unsupported languages fall back to empty sets with a warning
- Auto-detected vs JSON/YAML by checking if input starts with an identifier, `~`, or `(`
- Pattern prefixes follow jj conventions: `exact:`, `substring:`, `glob:`, `regex:`

**Why:** User wants human-friendly hunk selection that aligns with jj's fileset/revset design philosophy, rather than requiring JSON objects.

**How to apply:** When extending the hunkset language, follow jj's algebraic set patterns (union `|`, intersection `&`, difference `~`, negation `~x`). Keep function signatures consistent with filesets.

**Next steps:** Add tree-sitter integration (src/semantic.rs) to populate `enclosing_function` and `enclosing_scope` metadata on enriched hunks.
