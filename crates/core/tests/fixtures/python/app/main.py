from app.util import used_helper, Service


def main():
    s = Service()
    s.run()
    print(used_helper())


def _dead_private():
    return 1


def _cycle_a():
    return _cycle_b()


def _cycle_b():
    return _cycle_a()


if __name__ == "__main__":
    main()
