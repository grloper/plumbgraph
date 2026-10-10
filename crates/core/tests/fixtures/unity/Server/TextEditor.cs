namespace Server
{
    internal class Editor { }

    // `Editor` is a generic name: outside a Unity file it is not UnityEditor.Editor.
    internal class TextEditor : Editor
    {
        void OnGUI() { }
        void OnInspectorGUI() { }
    }
}
