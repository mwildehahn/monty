use monty::{PyObject, RunSnapshot, StdPrint};

#[test]
fn simple_expression_completes() {
    let exec = RunSnapshot::new("x + 1".to_owned(), "test.py", &["x"], vec![]).unwrap();
    let result = exec.run_no_limits(vec![PyObject::Int(41)], &mut StdPrint).unwrap();
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(42));
}

#[test]
fn external_function_call_expression_statement() {
    // Calling an undefined function returns a FunctionCall variant
    let exec = RunSnapshot::new("foo(1, 2)".to_owned(), "test.py", &[], vec!["foo".to_string()]).unwrap();
    let progress = exec.run_no_limits(vec![], &mut StdPrint).unwrap();

    let (name, args, _kwargs, state) = progress.into_function_call().expect("function call");
    assert_eq!(name, "foo");
    assert_eq!(args, vec![PyObject::Int(1), PyObject::Int(2)]);

    // Resume with a return value - the value is returned (REPL behavior: last expression is returned)
    let result = state.run(PyObject::Int(100), &mut StdPrint).unwrap();
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(100));
}

#[test]
fn external_function_call_with_assignment() {
    // Test external function call in assignment: result = foo(1, 2)
    let exec = RunSnapshot::new(
        "
result = foo(1, 2)
result + 10"
            .to_owned(),
        "test.py",
        &[],
        vec!["foo".to_owned()],
    )
    .unwrap();
    let progress = exec.run_no_limits(vec![], &mut StdPrint).unwrap();

    let (name, args, _kwargs, state) = progress.into_function_call().expect("function call");
    assert_eq!(name, "foo");
    assert_eq!(args, vec![PyObject::Int(1), PyObject::Int(2)]);

    // Resume with return value - should be assigned to 'result'
    let result = state.run(PyObject::Int(32), &mut StdPrint).unwrap();
    // result + 10 = 32 + 10 = 42
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(42));
}

#[test]
fn external_function_call_no_args() {
    // Test external function call with no arguments
    let exec = RunSnapshot::new(
        "
x = get_value()
x"
        .to_owned(),
        "test.py",
        &[],
        vec!["get_value".to_owned()],
    )
    .unwrap();
    let progress = exec.run_no_limits(vec![], &mut StdPrint).unwrap();

    let (name, args, _kwargs, state) = progress.into_function_call().expect("function call");
    assert_eq!(name, "get_value");
    assert!(args.is_empty());

    let result = state.run(PyObject::String("hello".to_string()), &mut StdPrint).unwrap();
    assert_eq!(
        result.into_complete().expect("complete"),
        PyObject::String("hello".to_string())
    );
}

#[test]
fn multiple_external_function_calls() {
    // Test multiple external function calls in sequence
    let code = "
a = foo(1)
b = bar(2)
a + b";
    let exec = RunSnapshot::new(
        code.to_owned(),
        "test.py",
        &[],
        vec!["foo".to_owned(), "bar".to_owned()],
    )
    .unwrap();

    // First external call: foo(1)
    let (name, args, _kwargs, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("first call");
    assert_eq!(name, "foo");
    assert_eq!(args, vec![PyObject::Int(1)]);

    // Resume with foo's return value
    let progress = state.run(PyObject::Int(10), &mut StdPrint).unwrap();

    // Second external call: bar(2)
    let (name, args, _kwargs, state) = progress.into_function_call().expect("second call");
    assert_eq!(name, "bar");
    assert_eq!(args, vec![PyObject::Int(2)]);

    // Resume with bar's return value
    let result = state.run(PyObject::Int(20), &mut StdPrint).unwrap();
    // a + b = 10 + 20 = 30
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(30));
}

#[test]
fn external_function_call_with_builtin_args() {
    // Test external function call with builtin function results as arguments
    let exec = RunSnapshot::new("foo(len([1, 2, 3]))".to_owned(), "test.py", &[], vec!["foo".to_owned()]).unwrap();
    let progress = exec.run_no_limits(vec![], &mut StdPrint).unwrap();

    let (name, args, _kwargs, _) = progress.into_function_call().expect("function call");
    assert_eq!(name, "foo");
    // len([1, 2, 3]) = 3, so args should be [3]
    assert_eq!(args, vec![PyObject::Int(3)]);
}

#[test]
fn external_function_call_preserves_existing_variables() {
    // Test that external calls don't affect existing variables
    let code = "
x = 10
y = foo(x)
x + y";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["foo".to_owned()]).unwrap();

    let (_, args, _kwargs, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("function call");
    // foo receives x=10
    assert_eq!(args, vec![PyObject::Int(10)]);

    // Resume with return value
    let result = state.run(PyObject::Int(5), &mut StdPrint).unwrap();
    // x + y = 10 + 5 = 15
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(15));
}

#[test]
fn external_function_nested_calls() {
    // Test nested external function calls: foo(bar(42))
    let code = "foo(bar(42))";
    let exec = RunSnapshot::new(
        code.to_owned(),
        "test.py",
        &[],
        vec!["foo".to_owned(), "bar".to_owned()],
    )
    .unwrap();

    // First: inner call bar(42)
    let (name, args, _kwargs, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("function call");

    assert_eq!(name, "bar");
    assert_eq!(args, vec![PyObject::Int(42)]);

    let progress = state.run(PyObject::Int(43), &mut StdPrint).unwrap();

    // Second: outer call foo(43)
    let (name, args, _kwargs, state) = progress.into_function_call().expect("function call");

    assert_eq!(name, "foo");
    assert_eq!(args, vec![PyObject::Int(43)]);

    let result = state.run(PyObject::Int(44), &mut StdPrint).unwrap();
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(44));
}

#[test]
fn clone_executor_iter() {
    // Test that ExecutorIter can be cloned and both copies work independently
    let exec1 = RunSnapshot::new("foo(42)".to_owned(), "test.py", &[], vec!["foo".to_owned()]).unwrap();
    let exec2 = exec1.clone();

    // Run first executor
    let (name, args, _kwargs, state) = exec1
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("function call");
    assert_eq!(name, "foo");
    assert_eq!(args, vec![PyObject::Int(42)]);
    let result = state.run(PyObject::Int(100), &mut StdPrint).unwrap();
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(100));

    // Run second executor (clone) - should work independently
    let (name, args, _kwargs, state) = exec2
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("function call");
    assert_eq!(name, "foo");
    assert_eq!(args, vec![PyObject::Int(42)]);
    let result = state.run(PyObject::Int(200), &mut StdPrint).unwrap();
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(200));
}

#[test]
fn external_function_call_in_if_true_branch() {
    // Test function call inside if block when condition is true
    let code = "
x = 1
if x == 1:
    result = foo(10)
else:
    result = bar(20)
result";
    let exec = RunSnapshot::new(
        code.to_owned(),
        "test.py",
        &[],
        vec!["foo".to_owned(), "bar".to_owned()],
    )
    .unwrap();

    // Should call foo(10), not bar
    let (name, args, _kwargs, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("function call");
    assert_eq!(name, "foo");
    assert_eq!(args, vec![PyObject::Int(10)]);

    let result = state.run(PyObject::Int(100), &mut StdPrint).unwrap();
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(100));
}

#[test]
fn external_function_call_in_if_false_branch() {
    // Test function call inside else block when condition is false
    let code = "
x = 0
if x == 1:
    result = foo(10)
else:
    result = bar(20)
result";
    let exec = RunSnapshot::new(
        code.to_owned(),
        "test.py",
        &[],
        vec!["foo".to_owned(), "bar".to_owned()],
    )
    .unwrap();

    // Should call bar(20), not foo
    let (name, args, _kwargs, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("function call");
    assert_eq!(name, "bar");
    assert_eq!(args, vec![PyObject::Int(20)]);

    let result = state.run(PyObject::Int(200), &mut StdPrint).unwrap();
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(200));
}

#[test]
fn external_function_call_in_for_loop() {
    // Test function call inside for loop
    let code = "
total = 0
for i in range(3):
    total = total + get_value(i)
total";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["get_value".to_owned()]).unwrap();

    // First iteration: get_value(0)
    let (name, args, _kwargs, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("first call");
    assert_eq!(name, "get_value");
    assert_eq!(args, vec![PyObject::Int(0)]);
    let progress = state.run(PyObject::Int(10), &mut StdPrint).unwrap();

    // Second iteration: get_value(1)
    let (name, args, _kwargs, state) = progress.into_function_call().expect("second call");
    assert_eq!(name, "get_value");
    assert_eq!(args, vec![PyObject::Int(1)]);
    let progress = state.run(PyObject::Int(20), &mut StdPrint).unwrap();

    // Third iteration: get_value(2)
    let (name, args, _kwargs, state) = progress.into_function_call().expect("third call");
    assert_eq!(name, "get_value");
    assert_eq!(args, vec![PyObject::Int(2)]);
    let result = state.run(PyObject::Int(30), &mut StdPrint).unwrap();

    // total = 10 + 20 + 30 = 60
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(60));
}

#[test]
fn external_function_call_state_across_loop() {
    // Test that state persists correctly across loop iterations with function calls
    let code = "
results = []
for i in range(2):
    x = compute(i)
    results.append(x)
results";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["compute".to_owned()]).unwrap();

    // First iteration: compute(0)
    let (name, args, _kwargs, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("first call");
    assert_eq!(name, "compute");
    assert_eq!(args, vec![PyObject::Int(0)]);
    let progress = state.run(PyObject::String("a".to_string()), &mut StdPrint).unwrap();

    // Second iteration: compute(1)
    let (name, args, _kwargs, state) = progress.into_function_call().expect("second call");
    assert_eq!(name, "compute");
    assert_eq!(args, vec![PyObject::Int(1)]);
    let result = state.run(PyObject::String("b".to_string()), &mut StdPrint).unwrap();

    // results should be ["a", "b"]
    assert_eq!(
        result.into_complete().expect("complete"),
        PyObject::List(vec![
            PyObject::String("a".to_string()),
            PyObject::String("b".to_string())
        ])
    );
}

#[test]
fn external_function_call_with_kwargs() {
    // Test external function call with keyword arguments
    let exec = RunSnapshot::new("foo(a=1, b=2)".to_owned(), "test.py", &[], vec!["foo".to_string()]).unwrap();
    let progress = exec.run_no_limits(vec![], &mut StdPrint).unwrap();

    let (name, args, kwargs, state) = progress.into_function_call().expect("function call");
    assert_eq!(name, "foo");
    assert!(args.is_empty());
    assert_eq!(kwargs.len(), 2);
    // Check kwargs contain the right key-value pairs
    let kwargs_map: std::collections::HashMap<_, _> = kwargs.into_iter().collect();
    assert_eq!(
        kwargs_map.get(&PyObject::String("a".to_string())),
        Some(&PyObject::Int(1))
    );
    assert_eq!(
        kwargs_map.get(&PyObject::String("b".to_string())),
        Some(&PyObject::Int(2))
    );

    let result = state.run(PyObject::Int(100), &mut StdPrint).unwrap();
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(100));
}

#[test]
fn external_function_call_with_mixed_args_and_kwargs() {
    // Test external function call with both positional and keyword arguments
    let exec = RunSnapshot::new(
        "foo(1, 2, x=3, y=4)".to_owned(),
        "test.py",
        &[],
        vec!["foo".to_string()],
    )
    .unwrap();
    let progress = exec.run_no_limits(vec![], &mut StdPrint).unwrap();

    let (name, args, kwargs, state) = progress.into_function_call().expect("function call");
    assert_eq!(name, "foo");
    assert_eq!(args, vec![PyObject::Int(1), PyObject::Int(2)]);
    assert_eq!(kwargs.len(), 2);
    let kwargs_map: std::collections::HashMap<_, _> = kwargs.into_iter().collect();
    assert_eq!(
        kwargs_map.get(&PyObject::String("x".to_string())),
        Some(&PyObject::Int(3))
    );
    assert_eq!(
        kwargs_map.get(&PyObject::String("y".to_string())),
        Some(&PyObject::Int(4))
    );

    let result = state.run(PyObject::Int(10), &mut StdPrint).unwrap();
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(10));
}

#[test]
fn external_function_call_kwargs_in_assignment() {
    // Test external function call with kwargs in assignment context
    let code = "
result = fetch(url='http://example.com', timeout=30)
result";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["fetch".to_owned()]).unwrap();

    let (name, args, kwargs, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("function call");
    assert_eq!(name, "fetch");
    assert!(args.is_empty());
    let kwargs_map: std::collections::HashMap<_, _> = kwargs.into_iter().collect();
    assert_eq!(
        kwargs_map.get(&PyObject::String("url".to_string())),
        Some(&PyObject::String("http://example.com".to_string()))
    );
    assert_eq!(
        kwargs_map.get(&PyObject::String("timeout".to_string())),
        Some(&PyObject::Int(30))
    );

    let result = state
        .run(PyObject::String("response".to_string()), &mut StdPrint)
        .unwrap();
    assert_eq!(
        result.into_complete().expect("complete"),
        PyObject::String("response".to_string())
    );
}

// === Nested control flow edge cases ===
// These tests verify correct behavior when external calls are combined with
// nested control flow structures (if/for). The key concern is that return values
// from external calls are correctly associated with the right call site.

#[test]
fn nested_if_with_external_calls_in_conditions() {
    // Test nested if statements where each condition involves an external call.
    // This verifies that return values don't get mixed up between outer and inner conditions.
    let code = "
result = 'none'
if check(1) == 1:
    if check(2) == 2:
        result = 'inner'
    else:
        result = 'outer_only'
else:
    result = 'failed'
result";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["check".to_owned()]).unwrap();

    // First call: check(1) in outer if condition
    let (name, args, _kwargs, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("first call");
    assert_eq!(name, "check");
    assert_eq!(args, vec![PyObject::Int(1)]);
    let progress = state.run(PyObject::Int(1), &mut StdPrint).unwrap();

    // Second call: check(2) in inner if condition
    let (name, args, _kwargs, state) = progress.into_function_call().expect("second call");
    assert_eq!(name, "check");
    assert_eq!(args, vec![PyObject::Int(2)]);
    let result = state.run(PyObject::Int(2), &mut StdPrint).unwrap();

    // Both conditions were true, so result should be 'inner'
    assert_eq!(
        result.into_complete().expect("complete"),
        PyObject::String("inner".to_string())
    );
}

#[test]
fn nested_if_external_call_inner_condition_false() {
    // Test nested if where outer condition is true but inner is false.
    // Verifies return values are correctly routed even when branches differ.
    let code = "
result = 'none'
if check(1) == 1:
    if check(2) == 999:
        result = 'inner'
    else:
        result = 'outer_only'
else:
    result = 'failed'
result";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["check".to_owned()]).unwrap();

    // First call: check(1) -> 1, outer condition true
    let (_, _, _, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("first call");
    let progress = state.run(PyObject::Int(1), &mut StdPrint).unwrap();

    // Second call: check(2) -> 2, but condition expects 999, so false
    let (_, _, _, state) = progress.into_function_call().expect("second call");
    let result = state.run(PyObject::Int(2), &mut StdPrint).unwrap();

    // Inner condition false, so result should be 'outer_only'
    assert_eq!(
        result.into_complete().expect("complete"),
        PyObject::String("outer_only".to_string())
    );
}

#[test]
fn triple_nested_if_with_external_calls() {
    // Test three levels of nested if statements with external calls.
    let code = "
result = 0
if get(1) == 1:
    if get(2) == 2:
        if get(3) == 3:
            result = 123
result";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["get".to_owned()]).unwrap();

    // First: get(1) -> 1
    let (_, args, _, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .unwrap();
    assert_eq!(args, vec![PyObject::Int(1)]);
    let progress = state.run(PyObject::Int(1), &mut StdPrint).unwrap();

    // Second: get(2) -> 2
    let (_, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(args, vec![PyObject::Int(2)]);
    let progress = state.run(PyObject::Int(2), &mut StdPrint).unwrap();

    // Third: get(3) -> 3
    let (_, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(args, vec![PyObject::Int(3)]);
    let result = state.run(PyObject::Int(3), &mut StdPrint).unwrap();

    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(123));
}

#[test]
fn for_loop_with_external_iterable() {
    // Test for loop where the iterable itself comes from an external call.
    let code = "
total = 0
for x in get_items():
    total = total + x
total";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["get_items".to_owned()]).unwrap();

    // get_items() returns the iterable
    let (name, args, _, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("function call");
    assert_eq!(name, "get_items");
    assert!(args.is_empty());

    // Return a list [10, 20, 30]
    let result = state
        .run(
            PyObject::List(vec![PyObject::Int(10), PyObject::Int(20), PyObject::Int(30)]),
            &mut StdPrint,
        )
        .unwrap();

    // total = 10 + 20 + 30 = 60
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(60));
}

#[test]
fn for_loop_external_iterable_and_body_call() {
    // Test for loop with external call for iterable AND external call in body.
    // Key behavior: the iterable is re-evaluated on each resume after a body pause.
    // This is correct for snapshot semantics where each resume re-executes from the
    // saved position. The iterable must return the same value on each call.
    //
    // With external iterables, the iterable is called EVERY time we need to continue
    // the loop (both after body pause and after body completion). This is because
    // we don't cache the iterable value - each resume re-evaluates from scratch.
    let code = "
total = 0
for x in get_items():
    total = total + process(x)
total";
    let exec = RunSnapshot::new(
        code.to_owned(),
        "test.py",
        &[],
        vec!["get_items".to_owned(), "process".to_owned()],
    )
    .unwrap();

    // First: get_items() returns [1, 2]
    let (name, _, _, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .unwrap();
    assert_eq!(name, "get_items");
    let progress = state
        .run(PyObject::List(vec![PyObject::Int(1), PyObject::Int(2)]), &mut StdPrint)
        .unwrap();

    // Second: process(1) in first iteration
    let (name, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(name, "process");
    assert_eq!(args, vec![PyObject::Int(1)]);
    let progress = state.run(PyObject::Int(10), &mut StdPrint).unwrap();

    // Third: process(2) in second iteration (skip index 0, resume at index 1)
    let (name, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(name, "process");
    assert_eq!(args, vec![PyObject::Int(2)]);
    let result = state.run(PyObject::Int(20), &mut StdPrint).unwrap();

    // Loop ends because index 2 is past the iterable length.
    // total = 10 + 20 = 30
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(30));
}

#[test]
fn nested_for_loops_with_external_calls() {
    // Test nested for loops with external calls in both loops.
    let code = "
results = []
for i in range(2):
    for j in range(2):
        results.append(compute(i, j))
results";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["compute".to_owned()]).unwrap();

    // compute(0, 0)
    let (_, args, _, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .unwrap();
    assert_eq!(args, vec![PyObject::Int(0), PyObject::Int(0)]);
    let progress = state.run(PyObject::String("00".to_string()), &mut StdPrint).unwrap();

    // compute(0, 1)
    let (_, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(args, vec![PyObject::Int(0), PyObject::Int(1)]);
    let progress = state.run(PyObject::String("01".to_string()), &mut StdPrint).unwrap();

    // compute(1, 0)
    let (_, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(args, vec![PyObject::Int(1), PyObject::Int(0)]);
    let progress = state.run(PyObject::String("10".to_string()), &mut StdPrint).unwrap();

    // compute(1, 1)
    let (_, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(args, vec![PyObject::Int(1), PyObject::Int(1)]);
    let result = state.run(PyObject::String("11".to_string()), &mut StdPrint).unwrap();

    assert_eq!(
        result.into_complete().expect("complete"),
        PyObject::List(vec![
            PyObject::String("00".to_string()),
            PyObject::String("01".to_string()),
            PyObject::String("10".to_string()),
            PyObject::String("11".to_string()),
        ])
    );
}

#[test]
fn if_inside_for_loop_with_external_calls() {
    // Test if statement inside for loop, both with external calls.
    let code = "
results = []
for i in range(3):
    if check(i) == i:
        results.append(i)
results";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["check".to_owned()]).unwrap();

    // check(0) -> 0 (condition true)
    let (_, args, _, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .unwrap();
    assert_eq!(args, vec![PyObject::Int(0)]);
    let progress = state.run(PyObject::Int(0), &mut StdPrint).unwrap();

    // check(1) -> 999 (condition false, won't append)
    let (_, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(args, vec![PyObject::Int(1)]);
    let progress = state.run(PyObject::Int(999), &mut StdPrint).unwrap();

    // check(2) -> 2 (condition true)
    let (_, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(args, vec![PyObject::Int(2)]);
    let result = state.run(PyObject::Int(2), &mut StdPrint).unwrap();

    // Only 0 and 2 passed the condition
    assert_eq!(
        result.into_complete().expect("complete"),
        PyObject::List(vec![PyObject::Int(0), PyObject::Int(2)])
    );
}

#[test]
fn for_loop_inside_if_with_external_condition() {
    // Test for loop inside if, where the if condition is an external call.
    let code = "
total = 0
if should_loop() == 1:
    for i in range(3):
        total = total + get_value(i)
total";
    let exec = RunSnapshot::new(
        code.to_owned(),
        "test.py",
        &[],
        vec!["should_loop".to_owned(), "get_value".to_owned()],
    )
    .unwrap();

    // should_loop() -> 1 (enter the if)
    let (name, _, _, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .unwrap();
    assert_eq!(name, "should_loop");
    let progress = state.run(PyObject::Int(1), &mut StdPrint).unwrap();

    // get_value(0)
    let (name, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(name, "get_value");
    assert_eq!(args, vec![PyObject::Int(0)]);
    let progress = state.run(PyObject::Int(10), &mut StdPrint).unwrap();

    // get_value(1)
    let (_, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(args, vec![PyObject::Int(1)]);
    let progress = state.run(PyObject::Int(20), &mut StdPrint).unwrap();

    // get_value(2)
    let (_, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(args, vec![PyObject::Int(2)]);
    let result = state.run(PyObject::Int(30), &mut StdPrint).unwrap();

    // total = 10 + 20 + 30 = 60
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(60));
}

#[test]
fn multiple_external_calls_in_single_expression() {
    // Test multiple external calls in a single expression: a() + b()
    // Both calls happen before the expression completes.
    let code = "add(1) + add(2)";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["add".to_owned()]).unwrap();

    // First: add(1)
    let (_, args, _, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .unwrap();
    assert_eq!(args, vec![PyObject::Int(1)]);
    let progress = state.run(PyObject::Int(10), &mut StdPrint).unwrap();

    // Second: add(2)
    let (_, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(args, vec![PyObject::Int(2)]);
    let result = state.run(PyObject::Int(20), &mut StdPrint).unwrap();

    // 10 + 20 = 30
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(30));
}

#[test]
fn external_call_in_if_condition_with_multiple_calls() {
    // Test if condition with multiple external calls: if a() + b() == 30:
    let code = "
result = 0
if add(1) + add(2) == 30:
    result = 1
result";
    let exec = RunSnapshot::new(code.to_owned(), "test.py", &[], vec!["add".to_owned()]).unwrap();

    // First: add(1) in condition
    let (_, args, _, state) = exec
        .run_no_limits(vec![], &mut StdPrint)
        .unwrap()
        .into_function_call()
        .unwrap();
    assert_eq!(args, vec![PyObject::Int(1)]);
    let progress = state.run(PyObject::Int(10), &mut StdPrint).unwrap();

    // Second: add(2) in condition
    let (_, args, _, state) = progress.into_function_call().unwrap();
    assert_eq!(args, vec![PyObject::Int(2)]);
    let result = state.run(PyObject::Int(20), &mut StdPrint).unwrap();

    // 10 + 20 = 30, condition true
    assert_eq!(result.into_complete().expect("complete"), PyObject::Int(1));
}
