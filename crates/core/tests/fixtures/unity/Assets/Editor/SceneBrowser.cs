using UnityEditor;
using UnityEngine;

namespace Game.EditorTools
{
    // EditorWindow + IHasCustomMenu: the editor calls OnGUI and AddItemsToMenu.
    internal class SceneBrowser : EditorWindow, IHasCustomMenu
    {
        void OnGUI() { }

        public void AddItemsToMenu(GenericMenu menu) { }

        private void UnusedBrowserHelper() { }
    }
}
