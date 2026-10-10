using UnityEngine;

namespace UYF
{
    internal abstract class CharacterBase : MonoBehaviour { }
    // indirect Unity base through another file/type, resolved after the merge
    internal partial class Hero : CharacterBase { }
    internal partial class Hero { void LateUpdate() { } }
}
