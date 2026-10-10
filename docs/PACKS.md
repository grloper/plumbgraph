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
| C# | class, interface, struct, enum, record, method, constructor | `public` | `static Main`, `override`, constructors; `[Fact]`/`[Test]`/`[TestMethod]`-style attributes; Unity messages and attributes (below) | `using` is namespace based and namespaces are not indexed: `System.*`/`Microsoft.*` = stdlib, everything else `unresolved` |

### Unity (C#)

The engine calls MonoBehaviour messages by name, so tier-0 sees no caller. Rules (golden fixture: `crates/core/tests/fixtures/unity`, tests: `crates/core/tests/unity_entry_points.rs`):

- **Message methods are entry points** (`Awake`, `Start`, `Update`, `LateUpdate`, `FixedUpdate`, `OnEnable`, `OnDisable`, `OnDestroy`, `OnValidate`, `Reset`, `OnGUI`, `OnCollision*`/`OnTrigger*` and their `2D` variants, plus the rest of the list in `UNITY_MESSAGES` in `crates/core/src/extract.rs`) when they are non-static methods of a class that derives directly from `MonoBehaviour`, `ScriptableObject`, `NetworkBehaviour`, `StateMachineBehaviour` or `UIBehaviour` (`Editor`/`EditorWindow` only in a Unity file, see below), **or** that lives under `Assets/` and whose first base type is a class rather than an interface (`I` + capital letter, the .NET convention). The inheritance chain is not resolved, so `Enemy : Character : MonoBehaviour` is recognised this way, while `Pool : IDisposable` is not. The class is then live too.
- **Attributes are entry points** in a Unity file: `[RuntimeInitializeOnLoadMethod]`, `[InitializeOnLoadMethod]`, `[InitializeOnLoad]` (class), `[MenuItem]`, `[ContextMenu]`, `[DidReloadScripts]`, `[PostProcessBuild]`, `[PostProcessScene]`, `[OnOpenAsset]` (with or without the `Attribute` suffix). A Unity file is one under `Assets/` or `Packages/`, or whose code (not comments or strings) has a `using UnityEngine…`/`using UnityEditor…` directive or a `UnityEngine.`/`UnityEditor.` qualified name.
- **Public methods of such classes** stay findings but get a 0.25 penalty and the fp_risk "may be bound to a UnityEvent (e.g. Button.onClick), an Animation Event or SendMessage in a scene, prefab or animation clip, which are not indexed". Scenes, prefabs, animation clips and `.asset` files are not parsed, so plumb cannot tell a bound handler from dead code.
- `[SerializeField]` fields: C# fields are not extracted, so they are never reported (tested, so this is noticed if field extraction is added).
- Not covered: messages invoked through `SendMessage("Name")` / `Invoke("Name")` strings are only a string-mention penalty; `Editor`/`EditorWindow` subclasses outside a Unity file; no Unity project or editor was run to produce these rules (they come from the Unity scripting reference).

Not covered: fields/properties/events, local functions, Go package-level `var`, generics-aware resolution, partial classes across files, DI containers/reflection (only marker strings lower confidence), registry checks for Go/Maven/NuGet (`check-deps` skips these languages). Dead-code calls to interface-implementing methods without `@Override`/`override` are flagged *subclass method* (lower confidence), not proven live.
