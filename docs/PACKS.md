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
- **Callback interfaces** in a Unity file: methods of `ISerializationCallbackReceiver` (`OnBeforeSerialize`, `OnAfterDeserialize`), `IHasCustomMenu` (`AddItemsToMenu`) and the UI EventSystem handlers (`IPointerClickHandler.OnPointerClick`, `IDragHandler.OnDrag`, ...; full list in `UNITY_CALLBACK_INTERFACES`) are entry points when the class or struct lists that interface, whether or not it is a component.
- **Attributes are entry points** in a Unity file: `[RuntimeInitializeOnLoadMethod]`, `[InitializeOnLoadMethod]`, `[InitializeOnLoad]` (class), `[MenuItem]`, `[ContextMenu]`, `[DidReloadScripts]`, `[PostProcessBuild]`, `[PostProcessScene]`, `[OnOpenAsset]` (with or without the `Attribute` suffix). A Unity file is one under `Assets/` or `Packages/`, or whose code (not comments or strings) has a `using UnityEngine…`/`using UnityEditor…` directive or a `UnityEngine.`/`UnityEditor.` qualified name.
- **Public methods of such classes** stay findings but get a 0.25 penalty and the fp_risk "may be bound to a UnityEvent (e.g. Button.onClick), an Animation Event or SendMessage in a scene, prefab or animation clip, which are not indexed". Scenes, prefabs, animation clips and `.asset` files are not parsed, so plumb cannot tell a bound handler from dead code.
- `[SerializeField]` fields: C# fields are not extracted, so they are never reported (tested, so this is noticed if field extraction is added).
- **Partial classes:** a message on a `partial` class is an entry point when any part of the type declares a Unity base, including a base declared in another file. Parts are merged by type name within one `Assets/` root (multiple `.asmdef` assemblies in one root are not told apart); the other parts of the type become live too. The base chain is followed through classes in the index (`Hero : CharacterBase`, `CharacterBase : MonoBehaviour`). Outcomes: a Unity base is found: the message is an entry point (`dead-code --verbose` prints `unity-message@partial-merged`); every base resolved to a non-Unity class, or there is no base class: an ordinary method that can be reported (HIGH is possible); a base type that is not in the index: reported as MEDIUM at most with the reason `unity-message-name-but-host-type-unresolved`. Plain classes with an `Update(float dt)` and ordinary methods that merely contain `Update` are not entry points.
- **Editor:** `Editor` (custom inspectors), `EditorWindow` and `ScriptableWizard` bases (in a Unity file), their messages (`OnInspectorGUI`, `OnSceneGUI`, `OnGUI`, `Update`, `OnWizardCreate`, ...), `[MenuItem]`, `[InitializeOnLoad]`, `[InitializeOnLoadMethod]`, `[RuntimeInitializeOnLoadMethod]`. Static helpers that carry none of these attributes are not entry points.
- Not covered: messages invoked through `SendMessage("Name")` / `Invoke("Name")` strings are only a string-mention penalty; `Editor`/`EditorWindow` subclasses outside a Unity file; no Unity project or editor was run to produce these rules (they come from the Unity scripting reference).

Not covered: fields/properties/events, local functions, Go package-level `var`, generics-aware resolution, partial classes across files for anything but Unity messages, DI containers/reflection (only marker strings lower confidence), registry checks for Go/Maven/NuGet (`check-deps` skips these languages). #### Unity gaps

`dead-code` prints a note when it indexes C# under `Assets/`. What plumb cannot see:

| gap | effect | what to do |
|---|---|---|
| `.unity` scenes, `.prefab` files, `.asset` files | not parsed: a public method bound in a UnityEvent (e.g. `Button.onClick`) looks unused | public methods of Unity classes stay findings at low confidence (0.25 penalty, fp_risk) |
| `SendMessage` / `BroadcastMessage` / string `Invoke("Name")` / `StartCoroutine("Name")` | only a string-mention penalty (low confidence), no edge | grep the name before deleting |
| animation events (`.anim` / `.controller`) | not parsed | same |
| Addressables, `Resources.Load`, reflection | not parsed | same |
| `scip-dotnet` | tier-1 is opt-in (`plumb enrich`) and does not see engine callbacks either | engine-by-name rules still apply on top |
| multiple `.asmdef` assemblies | partial parts are merged per `Assets/` root, not per assembly | same-named types in different assemblies can merge |

Never mass-delete HIGH findings under `Assets/` without confirming engine callbacks and serialization.

Dead-code calls to interface-implementing methods without `@Override`/`override` are flagged *subclass method* (lower confidence), not proven live.
