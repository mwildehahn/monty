# Datetime Support Plan (Phase 1: Fixed Offset + Native Python Bindings)

## Goal
Implement a secure, minimal `datetime` feature set in Monty that enables:
1. Getting "now" through host-controlled OS callbacks.
2. Calculating today's date in a requested fixed-offset timezone.
3. Performing `datetime`/`date` and `timedelta` arithmetic.
4. End-to-end native Python tests through `pydantic_monty`.

This phase intentionally avoids broad stdlib parity and focuses on a small, defensible vertical slice.

## Constraints and Requirements
1. All current-time reads must go through an OS callback (same trust model as `os.environ`).
2. `date.today()` must be derived from the same `now` source as `datetime.now()`.
3. The same callback contract must be reusable for future `time.time()`.
4. Timezone support is fixed-offset only in this phase.
5. No filesystem/network/process access can be introduced by this work.
6. No `unsafe` Rust.

## Phase 1 Scope
1. `import datetime` supported.
2. Types:
- `datetime.date`
- `datetime.datetime`
- `datetime.timedelta`
- `datetime.timezone` (fixed offset only)
3. Constructors and classmethods:
- `date(year, month, day)`
- `datetime(year, month, day, hour=0, minute=0, second=0, microsecond=0, tzinfo=None)`
- `timedelta(days=0, seconds=0, microseconds=0, milliseconds=0, minutes=0, hours=0, weeks=0)`
- `timezone(offset, name=None)`
- `date.today()`
- `datetime.now(tz=None)`
4. Arithmetic:
- `datetime +/- timedelta`
- `datetime - datetime -> timedelta`
- `date +/- timedelta`
- `date - date -> timedelta`
- `timedelta +/- timedelta`
- unary `-timedelta`
5. Basic comparisons:
- same-type comparisons for `date`, `datetime`, `timedelta`
- enforce aware/naive rules for `datetime` subtraction/comparison
6. `timedelta.total_seconds()`.

## Explicit Non-Goals (Phase 1)
1. `time.time()` implementation (contract prepared, implementation deferred).
2. IANA zone names, DST transitions, zone database integration.
3. `fromtimestamp`, `utcfromtimestamp`, `strptime`, `fromisoformat`, `isoformat`.
4. `datetime.time` type and rich formatting APIs.
5. JS callback support for OS calls (JS currently does not support `RunProgress::OsCall` end-to-end).

## OS Callback Design
### New OS function
Add `OsFunction::DateTimeNow` serialized as `datetime.now`.

### Callback arguments
Use a single callback shape for all "current time" use cases:
- Function name: `datetime.now`
- Positional args:
  1. `mode: str` where `mode in {'datetime', 'date', 'timestamp'}`
  2. `offset_seconds: int | None` (requested fixed offset; `None` means local/default host policy)
- Keyword args: none for phase 1.

### Callback return values (phase 1)
1. For `mode='datetime'` or `mode='date'`: return Unix timestamp as float seconds (`float`).
2. For future `mode='timestamp'`: same float seconds contract.

Monty-side conversion:
1. If `tz=None`, interpret returned timestamp in host local/default policy for now.
2. If `tz=timezone(...)`, apply provided fixed offset on Monty side.
3. `date.today()` computes date from the same timestamp source via `datetime.now` callback path.

### Why this contract
1. Single source of truth for "now".
2. Reuses cleanly for future `time.time()`.
3. Keeps host API simple and deterministic for tests.

## Runtime Data Model
Implement new heap types in `crates/monty/src/types/`:
1. `Date`
- Stored as validated civil date components and/or days-from-epoch.
2. `DateTime`
- Stored as UTC instant plus optional fixed offset metadata (for aware values).
- Naive datetimes represented without offset.
3. `TimeDelta`
- Stored as normalized `(days, seconds, microseconds)` semantics.
4. `TimeZone`
- Stored as fixed offset seconds and optional display name.

Representation must make arithmetic/comparison cheap and deterministic.

## Rust Library Choice
1. Use `chrono` in phase 1 for calendar/offset math and normalization.
2. Keep parsing/formatting layer isolated so `speedate` can be added in phase 2 for CPython-style parsing behavior.

## Core Implementation Tasks
### 1) Module and symbol wiring
1. Add `datetime` module support in:
- `crates/monty/src/modules/mod.rs`
- `crates/monty/src/intern.rs` (static strings for module/type/method names)
2. Add `crates/monty/src/modules/datetime.rs`.

### 2) Type system integration
1. Extend `Type` enum for datetime types in `crates/monty/src/types/type.rs`.
2. Register new callable type IDs where needed for constructors.
3. Add new heap variants in `crates/monty/src/heap.rs`.
4. Export modules in `crates/monty/src/types/mod.rs`.

### 3) Behavior implementation
1. New files:
- `crates/monty/src/types/date.rs`
- `crates/monty/src/types/datetime.rs`
- `crates/monty/src/types/timedelta.rs`
- `crates/monty/src/types/timezone.rs`
2. Implement `PyTrait` methods needed for:
- `py_type`, `py_repr_fmt`, `py_eq`, `py_cmp`
- `py_add`, `py_sub`, `py_call_attr`, `py_getattr`
3. Implement classmethod behavior for `date.today` and `datetime.now` yielding `AttrCallResult::OsCall`.

### 4) VM call-path extension
1. Extend type classmethod dispatch in `crates/monty/src/bytecode/vm/call.rs` so type methods can return `OsCall` for builtin type objects (similar to existing attr call flow).
2. Ensure ref-count safety in all early-return/error paths using `defer_drop!` or `HeapGuard`.

### 5) OS function enum and dispatch
1. Add `DateTimeNow` in `crates/monty/src/os.rs`.
2. Ensure `RunProgress::OsCall` stringification yields `datetime.now` in bindings.

### 6) Arithmetic integration
1. Update `crates/monty/src/value.rs` binary operation routing so `Value::Ref` subtraction/addition can delegate to new type ops, not only current LongInt-specialized paths.
2. Preserve existing numeric behavior and avoid regressions.

### 7) Exception parity
1. Use existing exception helper patterns in `crates/monty/src/exception_private.rs`.
2. Add dedicated helper constructors for repeated datetime errors.
3. Match CPython error classes/messages for:
- invalid constructor ranges
- aware vs naive mismatch
- invalid timezone offset ranges

## Native Python Binding Tasks
### 1) Conversion layer
Update `crates/monty-python/src/convert.rs`:
1. `py_to_monty`:
- recognize Python `datetime.date`, `datetime.datetime`, `datetime.timedelta`, `datetime.timezone`
- convert to dedicated `MontyObject` variants (new variants required in core object model)
2. `monty_to_py`:
- reconstruct native Python datetime objects, not strings/tuples

### 2) Public object model
Update `crates/monty/src/object.rs`:
1. Add `MontyObject` variants for date/datetime/timedelta/timezone.
2. Add serialization and `to_value`/`from_value` conversion paths.

### 3) Python callback typing and helpers
1. Extend `OsFunction` literal in `crates/monty-python/python/pydantic_monty/os_access.py` with `'datetime.now'`.
2. Extend `AbstractOS.__call__` dispatch and interface docs.
3. Update stubs in `crates/monty-python/python/pydantic_monty/_monty.pyi` so the callback type includes new function name semantics.

## Typeshed / Type Checking Tasks
1. Add custom stub: `crates/monty-typeshed/custom/datetime.pyi` with minimal phase-1 API.
2. Include `datetime` in `VERSIONS` generation in `crates/monty-typeshed/update.py`.
3. Replace `missing_stdlib_datetime` expectation in `crates/monty-type-checking/tests/main.rs` with positive assertions.
4. Add/update type-check test fixture coverage for new datetime module symbols.

## Test Plan
### 1) Rust unit/integration tests
1. Extend `crates/monty/tests/os_tests.rs`:
- verify `date.today()` and `datetime.now(...)` yield `OsCall` with `OsFunction::DateTimeNow`
- verify argument payload (`mode`, `offset_seconds`)
2. Add arithmetic/unit tests for date/datetime/timedelta semantics and error cases.

### 2) Datatest harness
1. Update `crates/monty/tests/datatest_runner.rs` OS dispatcher:
- handle `DateTimeNow` deterministically with fixed timestamp fixtures
2. Update `scripts/iter_test_methods.py` with CPython-side equivalent behavior for `# call-external` tests.

### 3) Python test cases (core)
Add consolidated file `crates/monty/test_cases/datetime__core.py` with `# call-external` covering:
1. `date.today()` from virtual now.
2. `datetime.now(timezone.utc)` and non-zero fixed offsets.
3. date/datetime +/- timedelta operations.
4. datetime subtraction results.
5. aware vs naive exception exactness.

### 4) Python package tests
Add/extend `crates/monty-python/tests/test_os_calls.py`:
1. Snapshot of `MontySnapshot.function_name == 'datetime.now'`.
2. Resume with native Python `datetime`/timestamp return and assert Monty output uses native datetime classes.
3. End-to-end test of arithmetic in Monty with native datetime values crossing callback boundary.

## Validation Commands
Run after implementation:
1. `make format-rs`
2. `make lint-rs`
3. `make lint-py`
4. `make test-cases`
5. `make pytest` (or targeted `uv run pytest crates/monty-python/tests/test_os_calls.py`)
6. optional focused rust tests for os/datetime modules.

## Security Review Checklist
1. No direct wall-clock reads inside sandbox runtime; all now values come from callback.
2. No new host resource access paths besides explicit `RunProgress::OsCall`.
3. Callback argument validation enforces strict types/ranges.
4. No unbounded allocations from datetime operations.
5. Ref-count correctness verified on all error paths.

## Rollout Strategy
1. Land phase 1 as one feature-complete PR for fixed-offset + native Python bindings.
2. Keep JS behavior unchanged for now (explicitly unsupported OS calls in JS run loop remains).
3. Follow with phase 2 PR for `time.time()`, parsing/formatting, and broader stdlib parity.

## Phase 2 Preview (Not in this PR)
1. Implement `time.time()` using the same `datetime.now` callback contract (`mode='timestamp'`).
2. Add ISO parse/format support (`speedate` integration candidate).
3. Add richer constructors (`fromtimestamp`, UTC variants).
4. Evaluate IANA timezone/DST support with clear host policy boundaries.
