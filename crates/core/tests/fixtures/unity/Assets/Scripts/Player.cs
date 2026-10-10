using UnityEngine;
using UnityEngine.Events;

namespace Game
{
    // Attached to a GameObject in a scene: nothing in code references this class.
    public class Player : MonoBehaviour
    {
        [SerializeField] private float speed = 3f;
        [SerializeField] private UnityEvent onDied;

        private void Awake() { }
        void Start() { Init(); }
        void Update() { Move(); }
        void LateUpdate() { }
        void FixedUpdate() { }
        void OnEnable() { }
        void OnDisable() { }
        void OnDestroy() { }
        void OnCollisionEnter(Collision c) { }
        void OnCollisionStay(Collision c) { }
        void OnCollisionExit(Collision c) { }
        void OnTriggerEnter(Collider other) { }
        void OnTriggerStay(Collider other) { }
        void OnTriggerExit(Collider other) { }
        void OnCollisionEnter2D(Collision2D c) { }
        void OnCollisionStay2D(Collision2D c) { }
        void OnCollisionExit2D(Collision2D c) { }
        void OnTriggerEnter2D(Collider2D other) { }
        void OnTriggerStay2D(Collider2D other) { }
        void OnTriggerExit2D(Collider2D other) { }
        void OnGUI() { }
        void OnValidate() { }
        void Reset() { }

        [ContextMenu("Respawn")]
        private void RespawnFromInspector() { }

        private void Init() { }
        private void Move() { transform.Translate(Vector3.forward * speed); }

        // Bound to Button.onClick in the scene (not indexed): only a low-confidence lead.
        public void Jump() { }

        // Real dead code: nothing calls it and Unity does not either.
        private void NeverCalled() { }
    }
}
