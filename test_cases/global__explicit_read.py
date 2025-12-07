# Using 'global' to read a global variable (though not strictly necessary for read)
x = 42


def f():
    global x
    return x


f()
# Return=42
