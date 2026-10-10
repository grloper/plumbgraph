using UnityEditor;
using UnityEngine;

namespace Game.EditorTools
{
    internal static class BuildTools
    {
        [MenuItem("Tools/Bake Lighting")]
        private static void Bake() { }

        [UnityEditor.MenuItemAttribute("Tools/Clear Cache")]
        private static void ClearCache() { }

        [RuntimeInitializeOnLoadMethod(RuntimeInitializeLoadType.BeforeSceneLoad)]
        private static void Boot() { }

        [InitializeOnLoadMethod]
        private static void OnEditorLoad() { }

        private static void OrphanTool() { }
    }

    [InitializeOnLoad]
    internal class StartupHook
    {
        static StartupHook() { }
    }
}
