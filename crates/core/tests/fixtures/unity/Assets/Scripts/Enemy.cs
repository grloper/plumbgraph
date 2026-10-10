namespace Game
{
    // Indirect MonoBehaviour (through Character, in another file): recognised because it
    // derives from something and lives under Assets/.
    internal class Enemy : Character
    {
        void Update() { Chase(); }
        void OnTriggerEnter(UnityEngine.Collider other) { }

        private void Chase() { }
        private void EnemyDeadHelper() { }
    }
}
