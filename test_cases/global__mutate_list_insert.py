# Insert into a global list from a function
items = ['a', 'c']


def insert_item():
    items.insert(1, 'b')


insert_item()
items
# Return=['a', 'b', 'c']
