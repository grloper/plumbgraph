using UnityEditor;
using UnityEngine;

namespace UYF.EditorTools
{
    internal class Inspector : Editor
    {
        public override void OnInspectorGUI() { }
        void OnSceneGUI() { }
        private void InspectorDeadHelper() { }
    }

    internal partial class Window : EditorWindow
    {
        void OnGUI() { }
        void Update() { }
    }

    internal partial class Window
    {
        void OnInspectorUpdate() { }
        void OnFocus() { }
    }

    internal class Wizard : ScriptableWizard
    {
        void OnWizardCreate() { }
        void OnWizardUpdate() { }
        void OnWizardOtherButton() { }
        private void WizardDeadHelper() { }
    }

    [InitializeOnLoad]
    internal static class Startup
    {
        static Startup() { }
        [MenuItem("UYF/Do")] static void Do() { }
        [InitializeOnLoadMethod] static void OnLoad() { }
        [RuntimeInitializeOnLoadMethod] static void Boot() { }
        static void UnrelatedStaticHelper() { }
    }
}
