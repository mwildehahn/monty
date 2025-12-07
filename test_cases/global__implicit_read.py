x = 42


def f():
    return x  # No local x, reads global


f()
# Return=42
