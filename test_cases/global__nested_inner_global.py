# Inner function uses 'global' to modify global variable
x = 1


def outer():
    def inner():
        global x
        x = 10

    inner()


outer()
x
# Return=10
