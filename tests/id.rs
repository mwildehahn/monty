use monty::{Executor, Exit};

macro_rules! id_tests {
    ($($name:ident: $code:literal, $expected:expr;)*) => {
        $(
            paste::item! {
                #[test]
                fn [< id_ $name >]() {
                    let mut ex = Executor::new($code, "test.py", &[]).unwrap();
                    let result = ex.run(vec![]);
                    let output = match result {
                        Ok(Exit::Return(obj)) => format!("{}: {}", obj.type_str(), obj.repr()),
                        otherwise => panic!("Unexpected exit: {:?}", otherwise),
                    };
                    let expected = $expected.trim_matches('\n');
                    assert_eq!(output, expected);
                }
            }
        )*
    }
}

id_tests! {
    // Singletons have constant IDs - always the same across calls
    singleton_none: "id(None) == id(None)", "bool: True";
    singleton_true: "id(True) == id(True)", "bool: True";
    singleton_false: "id(False) == id(False)", "bool: True";
    singleton_ellipsis: "id(...) == id(...)", "bool: True";
    singleton_distinct: "(id(None), id(True), id(False), id(...))", "tuple: (2, 3, 4, 1)";

    // Inline values (Int, Float) - each literal creates distinct boxed identity when id() called
    int_literals_distinct: "id(10) == id(20)", "bool: False";
    float_literals_distinct: "id(1.5) == id(2.5)", "bool: False";

    // Inline value assignment - copy creates separate object, boxed separately when id() called
    int_copy_distinct: "
x = 100
y = x
id(x) == id(y)
", "bool: False";

    // Heap-allocated values - each literal allocates separately
    str_literals_distinct: "id('hello') == id('hello')", "bool: False";
    list_literals_distinct: "id([1, 2]) == id([1, 2])", "bool: False";
    tuple_literals_distinct: "id((1, 2)) == id((1, 2))", "bool: False";
    bytes_literals_distinct: "id(b'test') == id(b'test')", "bool: False";

    // Variable assignment - heap-allocated values share identity when assigned
    str_assignment_shared: "
s = 'test'
t = s
id(s) == id(t)
", "bool: True";

    list_assignment_shared: "
lst = [1, 2]
ref = lst
id(lst) == id(ref)
", "bool: True";

    tuple_assignment_shared: "
t = (1, 2)
ref = t
id(t) == id(ref)
", "bool: True";

    bytes_assignment_shared: "
b = b'data'
ref = b
id(b) == id(ref)
", "bool: True";

    // Same variable always returns same ID
    var_stable: "
lst = [1, 2]
id(lst) == id(lst)
", "bool: True";

    // Mutation doesn't change ID
    list_mutate_stable: "
lst = [1]
old_id = id(lst)
lst.append(2)
old_id == id(lst)
", "bool: True";

    // Multiple references share ID
    multiple_refs: "
obj = [1, 2, 3]
r1 = obj
r2 = r1
(id(obj) == id(r1), id(r1) == id(r2))
", "tuple: (True, True)";

    // Different types have different IDs
    mixed_types: "id(1) == id('1')", "bool: False";
    bool_vs_int: "(id(True) == id(1), id(False) == id(0))", "tuple: (False, False)";
}
