using System;
using UnityEngine;
using UnityEngine.EventSystems;

namespace Game
{
    // Not a MonoBehaviour: Unity's serializer calls these because of the interface.
    [Serializable]
    internal class SaveData : ISerializationCallbackReceiver
    {
        public void OnBeforeSerialize() { }
        public void OnAfterDeserialize() { }
        public void UnusedSaveHelper() { }
    }

    // The EventSystem calls interface methods on components.
    internal class ClickTarget : MonoBehaviour, IPointerClickHandler, IDragHandler
    {
        public void OnPointerClick(PointerEventData eventData) { }
        public void OnDrag(PointerEventData eventData) { }
    }
}
