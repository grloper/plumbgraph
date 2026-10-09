# Language packs

A pack is a directory containing `pack.toml` plus tree-sitter query files:

```
packs/<id>/pack.toml   # extensions, manifests, registry, node kinds, markers, test globs
packs/<id>/defs.scm    # captures definitions (functions, classes, methods, ...)
packs/<id>/imports.scm # captures import statements
```

Built-in packs: `python`, `javascript`, `typescript`, `tsx`, `rust`, `go`, `java`, `csharp` (embedded in the binary). The loader (`crates/langs`) validates every pack (known grammar, queries compile, extensions unique) and fails with a clear message otherwise.

**v0.1 limitation:** a pack can only select one of the grammars compiled into the binary (`grammar = "python" | "javascript" | "typescript" | "tsx" | "rust" | "go" | "java" | "c-sharp"`). Loading arbitrary grammars at runtime is on the roadmap, not implemented. See `crates/langs/packs/python/pack.toml` for a commented example and `crates/langs/src/lib.rs` for the validation rules.

## Go, Java, C# (v1)

Tier-0 only (tree-sitter, name-based). What each pack knows:

| | definitions | "exported" | entry points / tests | imports |
|---|---|---|---|---|
| Go | func, method, struct, interface, type, const | capitalised name | `main`, `init`, well-known interface methods (`String`, `Error`, `ServeHTTP`, …); `*_test.go` | package path matched to a directory of the project (suffix match), else stdlib (no dot in first segment) / external |
| Java | class, interface, enum, record, annotation type, method, constructor | `public` (members also need an exported parent; interface members are public) | `static main`, `@Override`, constructors; `@Test`-style annotations, `src/test/**`; other annotations mark a symbol *decorated* (lower confidence) | qualified name matched to `a/b/C.java` or a package directory; `java.*`/`javax.*` stdlib |
| C# | class, interface, struct, enum, record, method, constructor | `public` | `static Main`, `override`, constructors; `[Fact]`/`[Test]`/`[TestMethod]`-style attributes | `using` is namespace based and namespaces are not indexed: `System.*`/`Microsoft.*` = stdlib, everything else `unresolved` |

Not covered: fields/properties/events, local functions, Go package-level `var`, generics-aware resolution, partial classes across files, DI containers/reflection (only marker strings lower confidence), registry checks for Go/Maven/NuGet (`check-deps` skips these languages). Dead-code calls to interface-implementing methods without `@Override`/`override` are flagged *subclass method* (lower confidence), not proven live.
