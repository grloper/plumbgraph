using System;

namespace Game
{
    // Under Assets/ but only implements interfaces: a C# class cannot reach MonoBehaviour
    // through an interface, so Unity never calls Update here.
    internal class Pooled : IDisposable, IComparable<Pooled>
    {
        public void Dispose() { }
        public int CompareTo(Pooled other) { return 0; }
        void Update() { }
    }
}
