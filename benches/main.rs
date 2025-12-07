use criterion::{black_box, criterion_group, criterion_main, Bencher, Criterion};
use monty::Executor;
use pprof::criterion::{Output, PProfProfiler};

use pyo3::prelude::*;

/// Benchmarks adding two numbers using Monty interpreter
fn add_two_monty(bench: &mut Bencher) {
    let ex = Executor::new("1 + 2", "test.py", &[]).unwrap();

    let r = ex.run(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, 3);

    bench.iter(|| {
        let r = ex.run(vec![]).unwrap();
        let int_value: i64 = r.as_ref().try_into().unwrap();
        black_box(int_value);
    });
}

/// Benchmarks adding two numbers using CPython
fn add_two_cpython(bench: &mut Bencher) {
    Python::with_gil(|py| {
        let fun: PyObject = PyModule::from_code(
            py,
            "def main():
                return 1 + 2
            ",
            "test.py",
            "main",
        )
        .unwrap()
        .getattr("main")
        .unwrap()
        .into();

        let r_py = fun.call0(py).unwrap();
        let r: i64 = r_py.extract(py).unwrap();
        assert_eq!(r, 3);

        bench.iter(|| {
            let r_py = fun.call0(py).unwrap();
            let r: i64 = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}

fn dict_set_get_monty(bench: &mut Bencher) {
    let ex = Executor::new(
        "
a = {}
a['key'] = 'value'
a['key']
        ",
        "test.py",
        &[],
    )
    .unwrap();

    let r = ex.run(vec![]).unwrap();
    let value: String = r.as_ref().try_into().unwrap();
    assert_eq!(value, "value");

    bench.iter(|| {
        let r = ex.run(vec![]).unwrap();
        let value: String = r.as_ref().try_into().unwrap();
        black_box(value);
    });
}

fn dict_set_get_cpython(bench: &mut Bencher) {
    Python::with_gil(|py| {
        let fun: PyObject = PyModule::from_code(
            py,
            "def main():
                a = {}
                a['key'] = 'value'
                return a['key']
            ",
            "test.py",
            "main",
        )
        .unwrap()
        .getattr("main")
        .unwrap()
        .into();

        let r_py = fun.call0(py).unwrap();
        let r: String = r_py.extract(py).unwrap();
        assert_eq!(r, "value");

        bench.iter(|| {
            let r_py = fun.call0(py).unwrap();
            let r: String = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}

fn list_append_monty(bench: &mut Bencher) {
    let ex = Executor::new(
        "
a = []
a.append(42)
a[0]
        ",
        "test.py",
        &[],
    )
    .unwrap();

    let r = ex.run(vec![]).unwrap();
    let value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(value, 42);

    bench.iter(|| {
        let r = ex.run(vec![]).unwrap();
        let value: i64 = r.as_ref().try_into().unwrap();
        black_box(value);
    });
}

/// Benchmarks adding two numbers using CPython
fn list_append_cpython(bench: &mut Bencher) {
    Python::with_gil(|py| {
        let fun: PyObject = PyModule::from_code(
            py,
            "def main():
                a = []
                a.append(42)
                return a[0]
            ",
            "test.py",
            "main",
        )
        .unwrap()
        .getattr("main")
        .unwrap()
        .into();

        let r_py = fun.call0(py).unwrap();
        let r: i64 = r_py.extract(py).unwrap();
        assert_eq!(r, 42);

        bench.iter(|| {
            let r_py = fun.call0(py).unwrap();
            let r: i64 = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}

// language=Python
const LOOP_MOD_13_CODE: &str = "
v = ''
for i in range(1_000):
    if i % 13 == 0:
        v += 'x'
len(v)
";

/// Benchmarks a loop with modulo operations using Monty interpreter
fn loop_mod_13_monty(bench: &mut Bencher) {
    let ex = Executor::new(LOOP_MOD_13_CODE, "test.py", &[]).unwrap();
    let r = ex.run(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, 77);

    bench.iter(|| {
        let r = ex.run(vec![]).unwrap();
        let int_value: i64 = r.as_ref().try_into().unwrap();
        black_box(int_value);
    });
}

/// Benchmarks a loop with modulo operations using CPython
fn loop_mod_13_cpython(bench: &mut Bencher) {
    Python::with_gil(|py| {
        let fun: PyObject = PyModule::from_code(
            py,
            // language=Python
            "def main():
                v = ''
                for i in range(1_000):
                    if i % 13 == 0:
                        v += 'x'
                return len(v)
            ",
            "test.py",
            "main",
        )
        .unwrap()
        .getattr("main")
        .unwrap()
        .into();

        let r = fun.call0(py).unwrap();
        let r: i64 = r.extract(py).unwrap();
        assert_eq!(r, 77);

        bench.iter(|| {
            let r_py = fun.call0(py).unwrap();
            let r: i64 = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}

/// Benchmarks end-to-end execution (parsing + running) using Monty
fn end_to_end_monty(bench: &mut Bencher) {
    bench.iter(|| {
        let ex = Executor::new(black_box("1 + 2"), "test.py", &[]).unwrap();
        let r = ex.run(vec![]).unwrap();
        let int_value: i64 = r.as_ref().try_into().unwrap();
        black_box(int_value);
    });
}

/// Benchmarks end-to-end execution (parsing + running) using CPython
fn end_to_end_cpython(bench: &mut Bencher) {
    Python::with_gil(|py| {
        bench.iter(|| {
            let fun: PyObject = PyModule::from_code(py, black_box("def main():\n  return 1 + 2"), "test.py", "main")
                .unwrap()
                .getattr("main")
                .unwrap()
                .into();
            let r_py = fun.call0(py).unwrap();
            let r: i64 = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}

/// Comprehensive benchmark exercising most supported Python features in one test.
/// Code is shared with test_cases/bench__kitchen_sink.py
/// Expected result: 3 + 1 + 10 + 1 + 1 + 3 + 11 + 7 + 21 = 58
const KITCHEN_SINK_CODE: &str = include_str!("../test_cases/bench__kitchen_sink.py");

/// Benchmarks comprehensive feature coverage using Monty interpreter
fn kitchen_sink_monty(bench: &mut Bencher) {
    let ex = Executor::new(KITCHEN_SINK_CODE, "test.py", &[]).unwrap();
    let r = ex.run(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, 58);

    bench.iter(|| {
        let r = ex.run(vec![]).unwrap();
        let int_value: i64 = r.as_ref().try_into().unwrap();
        black_box(int_value);
    });
}

/// Wraps test case code in a function for CPython execution.
/// Filters out test metadata comments and adds proper indentation.
fn wrap_for_cpython(code: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut last_expr = String::new();

    for line in code.lines() {
        // Skip test metadata comments
        if line.starts_with("# Return=") || line.starts_with("# Raise=") || line.starts_with("# skip=") {
            continue;
        }
        // Track the last non-empty, non-comment line as potential return expression
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            last_expr = line.to_string();
        }
        lines.push(format!("    {line}"));
    }

    // Replace last expression with return statement
    if let Some(last) = lines.iter().rposition(|l| l.trim() == last_expr.trim()) {
        lines[last] = format!("    return {}", last_expr.trim());
    }

    format!("def main():\n{}", lines.join("\n"))
}

/// Benchmarks comprehensive feature coverage using CPython
fn kitchen_sink_cpython(bench: &mut Bencher) {
    Python::with_gil(|py| {
        let code = wrap_for_cpython(KITCHEN_SINK_CODE);
        let fun: PyObject = PyModule::from_code(py, &code, "test.py", "main")
            .unwrap()
            .getattr("main")
            .unwrap()
            .into();

        let r_py = fun.call0(py).unwrap();
        let r: i64 = r_py.extract(py).unwrap();
        assert_eq!(r, 58);

        bench.iter(|| {
            let r_py = fun.call0(py).unwrap();
            let r: i64 = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}

/// Configures all benchmark groups
fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_two");
    group.bench_function("monty", add_two_monty);
    group.bench_function("cpython", add_two_cpython);
    group.finish();

    let mut group = c.benchmark_group("dict_set_get");
    group.bench_function("monty", dict_set_get_monty);
    group.bench_function("cpython", dict_set_get_cpython);
    group.finish();

    let mut group = c.benchmark_group("list_append");
    group.bench_function("monty", list_append_monty);
    group.bench_function("cpython", list_append_cpython);
    group.finish();

    let mut group = c.benchmark_group("loop_mod_13");
    group.bench_function("monty", loop_mod_13_monty);
    group.bench_function("cpython", loop_mod_13_cpython);
    group.finish();

    let mut group = c.benchmark_group("end_to_end");
    group.bench_function("monty", end_to_end_monty);
    group.bench_function("cpython", end_to_end_cpython);
    group.finish();

    let mut group = c.benchmark_group("kitchen_sink");
    group.bench_function("monty", kitchen_sink_monty);
    group.bench_function("cpython", kitchen_sink_cpython);
    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = criterion_benchmark
);
criterion_main!(benches);
