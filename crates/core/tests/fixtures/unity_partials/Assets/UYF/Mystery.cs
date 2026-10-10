namespace UYF
{
    // partial with a base class that is not in the index and not a Unity base: unresolved
    internal partial class Mystery : ExternalLibBase { void Update() { } }
    internal partial class Mystery { void Other() { } }
}
