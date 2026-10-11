from app import answer

def test_answer():
    actual = answer()
    assert actual == 42
