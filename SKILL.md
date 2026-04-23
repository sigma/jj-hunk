---
name: jj-hunk
description: Programmatic hunk selection for jj (Jujutsu). Use when splitting commits, making partial commits, selectively squashing changes, or coordinating concurrent agent work into clean history.
---

# jj-hunk: Programmatic Hunk Selection

Use `jj-hunk` for non-interactive hunk selection in jj. Essential for AI agents that need to create clean, logical commits from mixed changes — especially when multiple agents work concurrently.

## When to Use This Skill

- Splitting a commit into multiple logical commits
- Committing only specific hunks (partial commit)
- Squashing only certain changes into parent
- Coordinating concurrent agent work into clean, independent branches
- Any hunk selection that would normally require `jj split -i` or `jj squash -i`

## Setup

```bash
cargo install jj-hunk
```

Add to `~/.jjconfig.toml`:
```toml
[merge-tools.jj-hunk]
program = "jj-hunk"
edit-args = ["select", "$left", "$right"]
```

## Hunkset Query Language

jj-hunk supports an algebraic query language (hunkset) for selecting hunks, inspired by jj's filesets and revsets. This is the recommended way to select hunks — it's more readable, composable, and semantically aware than JSON specs.

### Operators

| Operator | Meaning | Example |
|----------|---------|---------|
| `x \| y` | Union | `type(insert) \| type(delete)` |
| `x & y` | Intersection | `type(insert) & glob("src/**")` |
| `x ~ y` | Difference | `all() ~ type(delete)` |
| `~x` | Negation | `~type(delete)` |
| `(x)` | Grouping | `(type(insert) \| type(replace)) & file("x")` |

### Functions

**File predicates:**

| Function | Description |
|----------|-------------|
| `file("path")` | Exact file path match |
| `glob("src/**/*.rs")` | Glob pattern on file path |
| `extension("rs")` | File extension |
| `status(modified)` | File status (modified, added, removed, renamed, copied) |

**Hunk type:**

| Function | Description |
|----------|-------------|
| `type(insert)` | Insertions only |
| `type(delete)` | Deletions only |
| `type(replace)` | Replacements only |

**Content matching:**

| Function | Description |
|----------|-------------|
| `content("text")` | Added or removed text contains "text" |
| `added("text")` | Added text contains "text" |
| `removed("text")` | Removed text contains "text" |

**Line ranges:**

| Function | Description |
|----------|-------------|
| `lines(10..20)` | Hunks touching lines 10-20 |

**Identity (stable across concurrent changes):**

| Function | Description |
|----------|-------------|
| `id("hunk-7c3d...")` | Select by stable hunk ID (supports prefix matching) |
| `all()` / `none()` | Everything / nothing |

**Semantic (tree-sitter powered, requires `semantic` feature):**

| Function | Description |
|----------|-------------|
| `function("name")` | Hunks inside a function/method |
| `scope("ClassName")` | Hunks inside a class/struct/impl/module |
| `annotation("test")` | Hunks in annotated/decorated functions |
| `doc()` | Hunks that are doc comments |
| `import()` | Hunks that are import/use/require statements |
| `toplevel()` | Hunks not inside any function or scope |
| `depth(0..1)` | Hunks at nesting depth 0 or 1 |

**Pattern prefixes** (on string arguments):
- Bare identifier: exact match — `type(insert)`
- Quoted string: substring match — `added("TODO")`
- `exact:"text"`, `substring:"text"`, `glob:"pattern"`, `regex:"pattern"`

### Examples

```bash
# All insertions in Rust files
jj-hunk split 'type(insert) & glob("src/**/*.rs")' "add new code"

# Everything inside UserService class
jj-hunk split 'scope("UserService")' "refactor: update UserService"

# All test functions
jj-hunk split 'annotation("test")' "test: add unit tests"

# Imports only
jj-hunk split 'import()' "chore: update imports"

# Everything except docs
jj-hunk split 'all() ~ doc()' "feat: implementation"
```

## Output Formats

```bash
jj-hunk list --format json    # Structured data (default)
jj-hunk list --format yaml    # YAML variant
jj-hunk list --format text    # Human-readable summary with semantic context
jj-hunk list --format diff    # Unified diff with hunk IDs and semantic context
```

The `diff` format produces a unified patch annotated with hunk IDs and enclosing function/scope names:

```diff
--- a/src/commands.rs
+++ b/src/commands.rs
@@ -211,1 +211,2 @@ list [hunk-162b7798da21...]
-        if !include.is_empty() && !matches_any(&include, &fh.path) {
+        let paths_to_check = fh.all_paths();
+        if !include.is_empty() && !paths_to_check.iter().any(|p| matches_any(&include, p)) {
```

This format is useful when you need to apply changes outside a pure jj workflow (e.g., `git am`), or when you want to inspect or archive the exact patch before applying.

## Core Workflow

### 1. Explore hunks with hunkset queries

```bash
# See all hunks
jj-hunk list --format text

# Preview what a query would select
jj-hunk list --spec 'scope("UserService")' --format text

# Verify a selection covers everything you expect
jj-hunk list --spec 'scope("UserService") ~ id("hunk-7c3d...")' --format text
# Empty output = the id covers everything in that scope
```

### 2. Select and split

Use hunkset expressions directly with split/commit/squash:

```bash
# Split by semantic scope
jj-hunk split 'scope("UserService") & ~import()' "refactor: update UserService"

# Split by file pattern
jj-hunk split 'glob("src/api/**")' "feat: add API endpoints"

# Split by content
jj-hunk split 'added("TODO") | added("FIXME")' "chore: add TODOs"
```

### 3. Use stable IDs for safety

When other changes may arrive concurrently, **always resolve your hunkset query to stable IDs before executing the split**. This protects against new hunks appearing between the query and the split:

```bash
# Step 1: Query to find the hunks you want
jj-hunk list --spec 'function("handle_request") & glob("src/api/**")' --format json

# Step 2: Note the hunk IDs from the output, then split using IDs
jj-hunk split 'id("hunk-7c3d...", "hunk-9a2b...", "hunk-ff01...")' "feat: handle_request implementation"
```

Hunk IDs are stable SHA256 hashes of the hunk content — they won't change even if other hunks are added to the same file by concurrent work.

### 4. JSON specs (alternative)

For complex selections or when building specs programmatically:

```json
{
  "files": {
    "src/foo.rs": {"ids": ["hunk-7c3d...", "hunk-2f91..."]},
    "src/bar.rs": {"action": "keep"},
    "src/qux.rs": {"action": "reset"}
  },
  "default": "reset"
}
```

Hunkset expressions and JSON specs are auto-detected — use whichever is clearer for the situation.

## Multi-Agent Concurrent Workflow

When multiple agents work in parallel on different tasks, their changes tend to intermingle in the working copy. jj-hunk enables each agent to retrospectively extract its own changes into clean, independent branches.

### Setup: Create the merge structure

Before agents start working, establish the topology:

```bash
# Create the merge point that will combine all agent work
jj new main -m "merge: combine agent work"
# Save its change ID
MERGE=$(jj log -r @ --template 'change_id' --no-graph)
```

### Each agent's workflow

Each agent works at the head (or a descendant of the merge), then cleans up:

```bash
# 1. Make changes normally (code, tests, etc.)
#    Changes land in the working copy alongside other agents' work.

# 2. Query to identify YOUR changes using semantic context
jj-hunk list --spec 'scope("UserService") & glob("src/api/**")' --format text

# 3. Verify the query captures exactly your work — refine if needed
#    Combine predicates for precision:
jj-hunk list --spec 'function("handle_request") & file("src/api/handler.rs")' --format text

# 4. Resolve to stable IDs (protects against concurrent changes)
#    Collect the hunk IDs from the output above.

# 5. Split your changes into a clean commit
jj-hunk split 'id("hunk-7c3d...", "hunk-9a2b...")' "feat: add request handler"

# 6. Rebase the clean commit to its proper place in the graph
jj rebase -r <new_change> -d main

# 7. Update the merge to include your branch
jj rebase -r $MERGE -d <new_change> -d <other_branches...>
```

### Verification

After each agent finishes, verify the merge is clean:

```bash
jj diff -r $MERGE
# Should be empty (or contain only conflict resolutions)
```

If the merge has unexpected content, an agent's split was incomplete — use `jj-hunk list -r $MERGE` to see what's left and dispatch it.

### Example: Three agents working concurrently

```
main
├── agent-1: feat: add database schema
│   (created by: jj-hunk split 'glob("src/db/**")' ...)
├── agent-2: feat: add API endpoints  
│   (created by: jj-hunk split 'scope("Router") & glob("src/api/**")' ...)
├── agent-3: refactor: update shared utils
│   (created by: jj-hunk split 'function("parse_config") | function("validate")' ...)
└── merge: combine agent work (should be empty)
```

### Key principles

- **Query first, split by ID**: Use hunkset queries to explore, but resolve to stable hunk IDs before executing the split. IDs are content-addressed (SHA256) and immune to concurrent modifications.
- **Combine predicates for precision**: Use `&` to intersect scope and file queries — `scope("MyClass") & file("src/models.rs")` is safer than either alone, because it won't accidentally capture unrelated changes that happen to be in the same file or an identically-named scope in a different file.
- **Verify the merge**: After splitting, the merge change should be empty. If it's not, something was missed — use `jj-hunk list -r $MERGE` to find and dispatch remaining hunks.
- **Rebase, don't move**: After `jj-hunk split` creates a new change, use `jj rebase` to position it in the graph. The split creates the change as a child of the current revision; rebasing moves it to its logical location.

## Tips

- **Prefer hunkset over JSON**: Hunkset expressions are more readable and composable. Reserve JSON specs for programmatic generation.
- **Use `--format text` for exploration**: Shows semantic context (enclosing function/scope) inline, making it easy to identify which hunks belong to which logical change.
- **Use `--format diff` for export**: Produces a unified patch with hunk IDs that can be applied via `git am` or archived for review.
- **Prefer IDs for stability**: Hunk IDs (SHA256) are immune to concurrent changes. Always resolve queries to IDs before executing splits in concurrent workflows.
- **Use `--spec` on `list` to verify**: Preview what a split would select before executing it. An empty result means your query matches nothing; refine it.
- **`"default": "reset"` is safer**: Explicitly include what you want rather than excluding what you don't.
