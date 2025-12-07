# Inner function reads global variable without 'global' keyword
x = 42


def outer():
    def inner():
        return x  # Reads global x

    return inner()


outer()
# Return=42
