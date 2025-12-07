# Using 'global' to write to a global variable
x = 1


def f():
    global x
    x = 2


f()
x
# Return=2
