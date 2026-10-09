import sys


def _dyn_target():
    return 4


def dispatch(name):
    return getattr(sys.modules[__name__], "_dyn_target")()
