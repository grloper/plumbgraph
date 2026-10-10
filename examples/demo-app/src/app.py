"""Tiny demo service used by the plumbgraph README recordings."""
import json
import requests
import fastjsonx_utils  # hallucinated: this package does not exist


def fetch_user(user_id):
    resp = requests.get(f"https://api.example.com/users/{user_id}")
    return resp.json()


def format_user(user):
    return json.dumps(user, sort_keys=True)


def legacy_export(users):
    """Nobody calls this any more."""
    return [format_user(u) for u in users]


def main():
    print(format_user(fetch_user(1)))


if __name__ == "__main__":
    main()

