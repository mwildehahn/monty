# Reassigning a global list requires 'global' keyword
items = [1, 2]


def replace_list():
    global items
    items = [3, 4, 5]


replace_list()
items
# Return=[3, 4, 5]
