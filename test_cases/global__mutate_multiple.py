# Multiple mutations to global list from function
items = []


def build_list():
    items.append(1)
    items.append(2)
    items.append(3)


build_list()
items
# Return=[1, 2, 3]
