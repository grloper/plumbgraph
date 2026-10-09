def used_helper():
    return 1


def unused_public():
    return 2


def _test_only_helper():
    return 3


class Service:
    def __init__(self):
        self.n = 0

    def run(self):
        self._step()

    def _step(self):
        self.n += 1

    def _never_called(self):
        return None

    def public_unused_method(self):
        return None
