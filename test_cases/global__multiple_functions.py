# Multiple functions sharing a global variable
counter = 0


def inc():
    global counter
    counter = counter + 1


def get():
    return counter


inc()
inc()
get()
# Return=2
