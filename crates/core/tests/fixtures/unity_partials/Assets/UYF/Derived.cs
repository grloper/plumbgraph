namespace UYF
{
    internal class PlainBase { }

    // non-component class with a (resolved, non-Unity) base class under Assets/
    internal class DerivedState : PlainBase
    {
        void Update(float dt) { }
    }
}
