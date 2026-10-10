using System;

namespace Server
{
    // Not Unity: outside Assets/, no Unity base type. Same method names must still be reported.
    // (Mentioning UnityEngine or UnityEditor in a comment must not make this a Unity file.)
    internal class Worker : IDisposable
    {
        private const string Note = "UnityEditor.MenuItem in a string does not count either";

        public void Dispose() { }
        void Start() { }
        void Update() { }
        [MenuItem]
        void Menu() { }
    }

    // Not reported: tier-0 resolves by name, so `[UnityEditor.MenuItemAttribute]` in
    // Assets/Editor/BuildTools.cs (a live method) links to this same-named class.
    internal class MenuItemAttribute : Attribute { }
}
