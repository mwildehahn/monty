# Mutating a global list doesn't require 'global' keyword
items = [1, 2]


def add_item():
    items.append(3)


add_item()
items
# Return=[1, 2, 3]
