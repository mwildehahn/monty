def f():
    x = 1
    global x  # type: ignore[reportAssignmentBeforeGlobalDeclaration]


f()
# ParseError=Exc: (<no-tb>) SyntaxError("name 'x' is assigned to before global declaration")
