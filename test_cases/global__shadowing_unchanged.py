# Verify global is unchanged after function shadows it
x = 10


def f():
    x = 99  # Local x
    return x


assert f() == 99  # Returns 99
assert x == 10  # Global should still be 10
