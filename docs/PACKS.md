# Language packs

A pack is a directory containing `pack.toml` plus tree-sitter query files:

```
packs/<id>/pack.toml   # extensions, manifests, registry, node kinds, markers, test globs
packs/<id>/defs.scm    # captures definitions (functions, classes, methods, ...)
packs/<id>/imports.scm # captures import statements
```

Built-in packs: `python`, `javascript`, `typescript`, `tsx`, `rust` (embedded in the binary). The loader (`crates/langs`) validates every pack (known grammar, queries compile, extensions unique) and fails with a clear message otherwise.

**v0.1 limitation:** a pack can only select one of the grammars compiled into the binary (`grammar = "python" | "javascript" | "typescript" | "tsx" | "rust"`). Loading arbitrary grammars at runtime is on the roadmap, not implemented. See `crates/langs/packs/python/pack.toml` for a commented example and `crates/langs/src/lib.rs` for the validation rules.
