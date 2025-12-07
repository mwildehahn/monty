# Mutating a global dict doesn't require 'global' keyword
data = {'a': 1}


def add_entry():
    data['b'] = 2


add_entry()
data
# Return={'a': 1, 'b': 2}
