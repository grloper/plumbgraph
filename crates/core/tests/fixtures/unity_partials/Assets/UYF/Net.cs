using UnityEngine;

namespace UYF
{
    // Unity component whose private non-message helper is truly unreferenced.
    internal partial class Net : MonoBehaviour
    {
        void Update() { }

        private static string LocalPortString() { return "0"; }
    }
}
