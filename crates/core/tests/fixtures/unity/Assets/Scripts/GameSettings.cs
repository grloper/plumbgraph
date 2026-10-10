using UnityEngine;

namespace Game
{
    [CreateAssetMenu(menuName = "Game/Settings")]
    public class GameSettings : ScriptableObject
    {
        void OnEnable() { }
        void OnValidate() { }

        private void StaleSettingsHelper() { }
    }
}
