from src.app import format_user


def test_format_user():
    assert format_user({"b": 1, "a": 2}) == '{"a": 2, "b": 1}'
