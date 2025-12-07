use ahash::AHashMap;
use monty::{Executor, RunError};
use pyo3::prelude::*;
use std::error::Error;
use std::fs;
use std::path::Path;

/// Specifies which interpreters a test should skip.
///
/// Parsed from an optional `# skip=monty,cpython` comment at the start of a test file.
/// If not present, defaults to running on both interpreters (both fields false).
#[derive(Debug, Clone, Default)]
struct TestSkips {
    monty: bool,
    cpython: bool,
}

/// Represents the expected outcome of a test fixture
#[derive(Debug, Clone)]
enum Expectation {
    /// Expect exception with specific message
    Raise(String),
    /// Expect parse error containing message
    ParseError(String),
    /// Expect successful execution, check py_str() output
    ReturnStr(String),
    /// Expect successful execution, check py_repr() output
    Return(String),
    /// Expect successful execution, check py_type() output
    ReturnType(String),
    /// Expect successful execution, check ref counts of named variables.
    /// Only used when `ref-counting` feature is enabled; skipped otherwise.
    RefCounts(#[cfg_attr(not(feature = "ref-counting"), allow(dead_code))] AHashMap<String, usize>),
    /// Expect successful execution without raising an exception (no return value check).
    /// Used for tests that rely on asserts or just verify code runs.
    NoException,
}

impl Expectation {
    /// Returns the expected value string
    fn expected_value(&self) -> &str {
        match self {
            Expectation::Raise(s)
            | Expectation::ParseError(s)
            | Expectation::ReturnStr(s)
            | Expectation::Return(s)
            | Expectation::ReturnType(s) => s,
            Expectation::RefCounts(_) | Expectation::NoException => "",
        }
    }
}

/// Parse a Python fixture file into code, expected outcome, and test skips.
///
/// The file may optionally start with a `# skip=monty,cpython` comment to specify
/// which interpreters to skip. If not present, defaults to running on both.
///
/// The file may have an expectation comment as the LAST line:
/// - `# Raise=ExceptionType('message')` - Exception format
/// - `# ParseError=message` - Parse error format
/// - `# Return.str=value` - Check py_str() output
/// - `# Return=value` - Check py_repr() output
/// - `# Return.type=typename` - Check py_type() output
/// - `# ref-counts={'var': count, ...}` - Check ref counts of named heap variables
///
/// If no expectation comment is present, the test just verifies the code runs without exception.
fn parse_fixture(content: &str) -> (String, Expectation, TestSkips) {
    let lines: Vec<&str> = content.lines().collect();

    assert!(!lines.is_empty(), "Empty fixture file");

    // Check for skip comment at the start of the file
    let (skips, code_start_idx) = if let Some(first_line) = lines.first() {
        if let Some(skip_str) = first_line.strip_prefix("# skip=") {
            let skips = TestSkips {
                monty: skip_str.contains("monty"),
                cpython: skip_str.contains("cpython"),
            };
            (skips, 1)
        } else {
            (TestSkips::default(), 0)
        }
    } else {
        (TestSkips::default(), 0)
    };

    // Check if first code line has an expectation (this is an error)
    if let Some(first_code_line) = lines.get(code_start_idx) {
        if first_code_line.starts_with("# Return")
            || first_code_line.starts_with("# Raise")
            || first_code_line.starts_with("# ParseError")
        {
            panic!("Expectation comment must be on the LAST line, not the first line");
        }
    }

    // Get the last line and check if it's an expectation comment
    let last_line = lines.last().unwrap();

    // Parse expectation from comment line if present
    // Note: Check more specific patterns first (Return.str, Return.type, ref-counts) before general Return
    let (expectation, code_lines) = if let Some(expected) = last_line.strip_prefix("# ref-counts=") {
        (
            Expectation::RefCounts(parse_ref_counts(expected)),
            &lines[code_start_idx..lines.len() - 1],
        )
    } else if let Some(expected) = last_line.strip_prefix("# Return.str=") {
        (
            Expectation::ReturnStr(expected.to_string()),
            &lines[code_start_idx..lines.len() - 1],
        )
    } else if let Some(expected) = last_line.strip_prefix("# Return.type=") {
        (
            Expectation::ReturnType(expected.to_string()),
            &lines[code_start_idx..lines.len() - 1],
        )
    } else if let Some(expected) = last_line.strip_prefix("# Return=") {
        (
            Expectation::Return(expected.to_string()),
            &lines[code_start_idx..lines.len() - 1],
        )
    } else if let Some(expected) = last_line.strip_prefix("# Raise=") {
        (
            Expectation::Raise(expected.to_string()),
            &lines[code_start_idx..lines.len() - 1],
        )
    } else if let Some(expected) = last_line.strip_prefix("# ParseError=") {
        (
            Expectation::ParseError(expected.to_string()),
            &lines[code_start_idx..lines.len() - 1],
        )
    } else {
        // No expectation comment - just run and check it doesn't raise
        (Expectation::NoException, &lines[code_start_idx..])
    };

    // Code is everything except the skip comment (and expectation comment if present)
    let code = code_lines.join("\n");

    (code, expectation, skips)
}

/// Parses the ref-counts format: {'var': count, 'var2': count2}
///
/// Supports both single and double quotes for variable names.
/// Example: {'x': 2, 'y': 1} or {"x": 2, "y": 1}
fn parse_ref_counts(s: &str) -> AHashMap<String, usize> {
    let mut counts = AHashMap::new();
    let trimmed = s.trim().trim_start_matches('{').trim_end_matches('}');
    for pair in trimmed.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let parts: Vec<&str> = pair.split(':').collect();
        assert!(
            parts.len() == 2,
            "Invalid ref-counts pair format: {pair}. Expected 'name': count"
        );
        let name = parts[0].trim().trim_matches('\'').trim_matches('"');
        let count: usize = parts[1]
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("Invalid ref count value: {}", parts[1]));
        counts.insert(name.to_string(), count);
    }
    counts
}

/// Run a test with the given code and expectation
///
/// This function executes Python code via the Executor and validates the result
/// against the expected outcome specified in the fixture.
fn run_test(path: &Path, code: &str, expectation: Expectation) {
    let test_name = path.strip_prefix("test_cases/").unwrap_or(path).display().to_string();

    // Handle ref-counting tests separately since they need run_ref_counts()
    #[cfg(feature = "ref-counting")]
    if let Expectation::RefCounts(expected) = &expectation {
        match Executor::new(code, "test.py", &[]) {
            Ok(ex) => {
                let result = ex.run_ref_counts(vec![]);
                match result {
                    Ok((_, (actual, unique_refs, heap_count))) => {
                        // Strict matching: verify all heap objects are accounted for by variables
                        assert_eq!(
                            unique_refs, heap_count,
                            "[{test_name}] Strict matching failed: {heap_count} heap objects exist, \
                             but only {unique_refs} are referenced by variables.\n\
                             Actual ref counts: {actual:?}"
                        );
                        assert_eq!(&actual, expected, "[{test_name}] ref-counts mismatch");
                    }
                    Err(e) => panic!("[{test_name}] Runtime error:\n{e}"),
                }
            }
            Err(parse_err) => {
                panic!("[{test_name}] Unexpected parse error: {parse_err:?}");
            }
        }
        return;
    }

    match Executor::new(code, "test.py", &[]) {
        Ok(ex) => {
            let result = ex.run(vec![]);
            match result {
                Ok(obj) => match expectation {
                    Expectation::ReturnStr(expected) => {
                        let output = obj.to_string();
                        assert_eq!(output, expected, "[{test_name}] str() mismatch");
                    }
                    Expectation::Return(expected) => {
                        let output = obj.py_repr();
                        assert_eq!(output, expected, "[{test_name}] py_repr() mismatch");
                    }
                    Expectation::ReturnType(expected) => {
                        let output = obj.type_name();
                        assert_eq!(output, expected, "[{test_name}] type_name() mismatch");
                    }
                    #[cfg(not(feature = "ref-counting"))]
                    Expectation::RefCounts(_) => {
                        // Skip ref-count tests when feature is disabled
                    }
                    Expectation::NoException => {
                        // Success - code ran without exception as expected
                    }
                    _ => panic!("[{test_name}] Expected return, got different expectation type"),
                },
                Err(e) => {
                    if let Expectation::Raise(expected) = expectation {
                        // Extract just the exception part without traceback
                        let output = match &e {
                            RunError::Exc(exc) => exc.exc.to_string(),
                            RunError::Internal(internal) => internal.to_string(),
                        };
                        assert_eq!(output, expected, "[{test_name}] Exception mismatch");
                    } else {
                        panic!("[{test_name}] Unexpected error:\n{e}");
                    }
                }
            }
        }
        Err(parse_err) => {
            if let Expectation::ParseError(expected) = expectation {
                let err_msg = parse_err.summary();
                assert_eq!(err_msg, expected, "[{test_name}] Parse error mismatch");
            } else {
                panic!("[{test_name}] Unexpected parse error: {parse_err:?}");
            }
        }
    }
}

/// Split Python code into statements and a final expression to evaluate.
///
/// For Return expectations, the last non-empty line is the expression to evaluate.
/// For Raise/NoException, the entire code is statements (returns None for expression).
///
/// Returns (statements_code, optional_final_expression).
fn split_code_for_module(code: &str, need_return_value: bool) -> (String, Option<String>) {
    let lines: Vec<&str> = code.lines().collect();

    // Find the last non-empty line
    let last_idx = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .expect("Empty code");

    if need_return_value {
        let last_line = lines[last_idx].trim();

        // Check if the last line is a statement (can't be evaluated as an expression)
        // Matches both `assert expr` and `assert(expr)` forms
        if last_line.starts_with("assert ") || last_line.starts_with("assert(") {
            // All code is statements, no expression to evaluate
            (lines[..=last_idx].join("\n"), None)
        } else {
            // Everything except last line is statements, last line is the expression
            let statements = lines[..last_idx].join("\n");
            let expr = last_line.to_string();
            (statements, Some(expr))
        }
    } else {
        // All code is statements (for exception tests or NoException)
        (lines[..=last_idx].join("\n"), None)
    }
}

/// Run a test through CPython to verify Monty produces the same output
///
/// This function executes the same Python code via CPython (using pyo3) and
/// compares the result with the expected value. This ensures Monty behaves
/// identically to CPython.
///
/// Code is executed at module level (not wrapped in a function) so that
/// `global` keyword semantics work correctly.
///
/// ParseError tests are skipped since Monty uses a different parser (ruff).
fn run_cpython_test(path: &Path, code: &str, expectation: &Expectation) {
    // Skip ParseError tests - Monty uses ruff parser which has different error messages
    if matches!(expectation, Expectation::ParseError(_) | Expectation::RefCounts(_)) {
        return;
    }

    let test_name = path.strip_prefix("test_cases/").unwrap_or(path).display().to_string();
    let need_return_value = matches!(
        expectation,
        Expectation::Return(_) | Expectation::ReturnStr(_) | Expectation::ReturnType(_)
    );
    let (statements, maybe_expr) = split_code_for_module(code, need_return_value);

    let result: Option<String> = Python::with_gil(|py| {
        // Execute statements at module level
        let globals = pyo3::types::PyDict::new(py);

        // Run the statements
        let stmt_result = py.run(&statements, Some(globals), None);

        // Handle exception during statement execution
        if let Err(e) = stmt_result {
            if matches!(expectation, Expectation::NoException) {
                panic!("[{test_name}] Expected no exception but got: {e}");
            }
            if matches!(expectation, Expectation::Raise(_)) {
                return Some(format_cpython_exception(py, &e));
            }
            panic!("[{test_name}] Unexpected CPython exception during statements: {e}");
        }

        // If we have an expression to evaluate, evaluate it
        if let Some(expr) = maybe_expr {
            match py.eval(&expr, Some(globals), None) {
                Ok(result) => {
                    // Code returned successfully - format based on expectation type
                    match expectation {
                        Expectation::Return(_) => Some(result.repr().unwrap().to_string()),
                        Expectation::ReturnStr(_) => Some(result.str().unwrap().to_string()),
                        Expectation::ReturnType(_) => Some(result.get_type().name().unwrap().to_string()),
                        Expectation::Raise(_) => {
                            panic!("[{test_name}] Expected exception but code completed normally")
                        }
                        Expectation::NoException | Expectation::ParseError(_) | Expectation::RefCounts(_) => {
                            unreachable!()
                        }
                    }
                }
                Err(e) => {
                    // Expression raised an exception
                    if matches!(expectation, Expectation::NoException) {
                        panic!("[{test_name}] Expected no exception but got: {e}");
                    }
                    if matches!(expectation, Expectation::Raise(_)) {
                        return Some(format_cpython_exception(py, &e));
                    }
                    panic!("[{test_name}] Unexpected CPython exception during eval: {e}");
                }
            }
        } else {
            // No expression to evaluate
            if matches!(expectation, Expectation::Raise(_)) {
                panic!("[{test_name}] Expected exception but code completed normally");
            }
            None // NoException expectation - success
        }
    });

    // Only compare if we have a result to compare
    if let Some(result) = result {
        assert_eq!(
            result,
            expectation.expected_value(),
            "[{test_name}] CPython result mismatch"
        );
    }
}

/// Format a CPython exception into the expected format.
fn format_cpython_exception(py: Python<'_>, e: &pyo3::PyErr) -> String {
    let exc_type = e.get_type(py).name().unwrap();
    let exc_message: String = e
        .value(py)
        .getattr("args")
        .and_then(|args| args.get_item(0))
        .and_then(pyo3::PyAny::extract)
        .unwrap_or_default();

    if exc_message.is_empty() {
        format!("{exc_type}()")
    } else if exc_message.contains('\'') {
        // Use double quotes when message contains single quotes (like Python's repr)
        format!("{exc_type}(\"{exc_message}\")")
    } else {
        // Use single quotes (default Python repr format)
        format!("{exc_type}('{exc_message}')")
    }
}

/// Test function that runs each fixture through Monty
fn run_test_cases_monty(path: &Path) -> Result<(), Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let (code, expectation, skips) = parse_fixture(&content);
    if !skips.monty {
        run_test(path, &code, expectation);
    }
    Ok(())
}

/// Test function that runs each fixture through CPython
fn run_test_cases_cpython(path: &Path) -> Result<(), Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let (code, expectation, skips) = parse_fixture(&content);
    if !skips.cpython {
        run_cpython_test(path, &code, &expectation);
    }
    Ok(())
}

// Generate tests for all fixture files using datatest-stable harness macro
datatest_stable::harness!(
    run_test_cases_monty,
    "test_cases",
    r"^.*\.py$",
    run_test_cases_cpython,
    "test_cases",
    r"^.*\.py$",
);
