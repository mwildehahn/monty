# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Monty is a sandboxed Python interpreter written in Rust. It parses Python code using Ruff's `ruff_python_parser` but implements its own runtime execution model for safety and performance. This is a work-in-progress project that currently supports a subset of Python features.

Project goals:

- **Safety**: Execute untrusted Python code safely without FFI or C dependencies, instead sandbox will call back to host to run foreign/external functions.
- **Performance**: Fast execution through compile-time optimizations and efficient memory layout
- **Simplicity**: Clean, understandable implementation focused on a Python subset
- **Snapshotting and iteration**: Plan is to allow code to be iteratively executed and snapshotted at each function call

## Build Commands

```bash
# format code and run clippy
make lint

# Build the project
cargo build
```

## Exception

It's important that exceptions raised/returned by this library match those raised by Python.

Wherever you see an Exception with a repeated message, create a dedicated method to create that exception `src/exceptions.rs`.

When writing exception messages, always check `src/exceptions.rs` for existing methods to generate that message.

## Code style

Avoid local imports, unless there's a very good reason, all imports should be at the top of the file.

IMPORTANT: every struct, enum and function should be a comprehensive but concise docstring to
explain what it does and why and any considerations or potential foot-guns of using that type.

The only exception is trait implementation methods where a docstring is not necessary if the method is self-explanatory.

Similarly, you should add lots of comments to code.

If you see a comment or docstring that's out of date - you MUST update it to be correct.

NOTE: COMMENTS AND DOCSTRINGS ARE EXTREMELY IMPORTANT TO THE LONG TERM HEALTH OF THE PROJECT.

## Tests

Do **NOT** write tests within modules unless explicitly prompted to do so.

Tests should live in the `tests/` directory.

Commands:

```bash
# Build the project
cargo build

# Run tests (this is the best way to run all tests as it enables the dec-ref-check feature)
make test

# Run a specific test
cargo test --features dec-ref-check execute_ok_add_ints

# Run the interpreter on a Python file
cargo run -- <file.py>
```

Tests should always be as concise as possible while covering all possible cases.

All Python execution behavior tests use file-based fixtures in `test_cases/`. File names: `<group_name>__<test_name>.py`. Unless it's completely obvious what is being tested, add short comments to the test code.

You may test behavior using multiple `assert` statements per file to avoid many small files, unless you're testing `assert` behavior, always add a message to the assert statement.

You should prefer single quotes for strings in python tests.

**Expectation formats** (on last line of file):
- `# Return=value` - Check `repr()` output
- `# Return.str=value` - Check `str()` output
- `# Return.type=typename` - Check `type()` output
- `# Raise=Exception('message')` - Expect exception
- `# ParseError=message` - Expect parse error
- `# ref-counts={...}` - To check reference counts of heap-allocated values
- No expectation comment - Just verify code runs without exception (useful for assert-based tests)

**Skip directive** (optional, on first line of file):
- `# skip=cpython` - Skip CPython test (only run on Monty)
- `# skip=monty` - Skip Monty test (only run on CPython)
- `# skip=monty,cpython` - Skip both (useful for temporarily disabling a test)

Run `make lint-py` after adding tests to format them.

Use make `make complete-tests` after adding tests with the expectations blank e.g. `# Return=` to fill in the expected value.

These tests are run via `datatest-stable` harness in `tests/datatest_runner.rs`.

## Reference Counting

Heap-allocated values (`Value::Ref`) use manual reference counting. Key rules:

- **Cloning**: Use `clone_with_heap(heap)` which increments refcounts for `Ref` variants.
- **Dropping**: Call `drop_with_heap(heap)` when discarding an `Value` that may be a `Ref`.
- **Borrow conflicts**: When you need to read from the heap and then mutate it, use `copy_for_extend()` to copy the `Value` without incrementing refcount, then call `heap.inc_ref()` separately after the borrow ends.

Container types (`List`, `Tuple`, `Dict`) also have `clone_with_heap()` methods.

## NOTES

ALWAYS run `make lint` after making changes and fix all suggestions to maintain code quality.

ALWAYS update this file when it is out of date.

NEVER write `unsafe` code, if you think you need to write unsafe code, explicitly ask the user or leave a `todo!()` with a suggestion and explanation.
