x = 10


def f():
    x = 20  # Creates LOCAL x (shadows global)
    return x


f()
# Return=20
