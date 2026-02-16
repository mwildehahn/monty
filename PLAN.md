# Datetime Support Plan (Phase 1: Fixed Offset + Native Python Bindings)

## Goal
Implement a secure, minimal `datetime` feature set in Monty that enables:
1. Getting "now" through host-controlled OS callbacks.
2. Calculating local `date.today()` and fixed-offset `datetime.now(tz=...)` values from that callback source.
3. Performing `datetime`/`date` and `timedelta` arithmetic.
4. End-to-end native Python tests through `pydantic_monty`.

This phase intentionally avoids broad stdlib parity and focuses on a small, defensible vertical slice.

## Progress Log (2026-02-11)
1. `[x]` Core runtime wiring complete:
- `datetime` module, interned names, builtin type registration, heap variants, and type exports integrated.
2. `[x]` Datetime type behavior complete:
- `date`, `datetime`, `timedelta`, and `timezone` constructors/classmethods/arithmetic/comparisons/repr-str implemented.
3. `[x]` VM + OS callback path complete:
- `call_type_method` refactor landed; `date.today()`/`datetime.now()` issue `DateTimeNow` OS calls and resume payload conversion works.
4. `[x]` Python binding/model integration complete:
- `MontyObject` datetime variants added; Rust↔Python conversion supports native datetime/date/timedelta/timezone objects.
5. `[x]` Typeshed/type-checking integration complete:
- Custom `datetime.pyi` added, vendor refresh integrated, and type-checking tests updated.
6. `[x]` Deterministic test harness complete:
- Datatest OS dispatcher + CPython iter shim updated for deterministic `datetime.now`/`date.today`.
7. `[x]` Validation complete:
- `make lint-rs`, `make lint-py`, `make test-cases`, `cargo test -p monty --test os_tests`, and targeted pytest OS files pass.

### Progress Note
- Timezone validation was aligned to current CPython behavior discovered during implementation:
  - second-level fixed offsets are accepted (not minute-only),
  - allowed range is strictly within ±24 hours.

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
3. Constants:
- `datetime.timezone.utc` (class attribute, equivalent to `timezone(timedelta(0))`)
4. Constructors and classmethods:
- `date(year, month, day)`
- `datetime(year, month, day, hour=0, minute=0, second=0, microsecond=0, tzinfo=None)`
- `timedelta(days=0, seconds=0, microseconds=0, milliseconds=0, minutes=0, hours=0, weeks=0)`
- `timezone(offset, name=None)`
- `date.today()`
- `datetime.now(tz=None)`
5. Arithmetic:
- `datetime +/- timedelta`
- `datetime - datetime -> timedelta`
- `date +/- timedelta`
- `date - date -> timedelta`
- `timedelta +/- timedelta`
- unary `-timedelta`
- `timedelta` bounds enforced: `-999999999 <= days <= 999999999` (matches CPython, prevents unbounded arithmetic chains)
6. Basic comparisons:
- same-type comparisons for `date`, `datetime`, `timedelta`
- enforce CPython aware/naive rules for `datetime`:
  - subtraction and ordering comparisons (`<`, `<=`, `>`, `>=`) between naive and aware raise `TypeError`
  - equality/inequality (`==`, `!=`) between naive and aware do not raise
7. `timedelta.total_seconds()`.
8. `__repr__` and `__str__` output must match CPython exactly:
- `date.__repr__`: `datetime.date(2024, 1, 15)`
- `date.__str__`: `2024-01-15`
- `datetime.__repr__`: `datetime.datetime(2024, 1, 15, 10, 30)` (omits trailing zeros)
- `datetime.__str__`: `2024-01-15 10:30:00`
- `timedelta.__repr__`: `datetime.timedelta(days=1, seconds=3600)`
- `timedelta.__str__`: `1 day, 1:00:00`
- `timezone.__repr__`: `datetime.timezone.utc` or `datetime.timezone(datetime.timedelta(seconds=3600))`

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
- Function name: `datetime.now`
- Positional args: none for phase 1.
- Keyword args: none for phase 1.

Note: `mode` was considered but dropped — all callers (`datetime.now()`, `date.today()`, future `time.time()`) use the same payload shape. Monty decides what to construct from it. If a future use case needs different host behavior, a new `OsFunction` variant can be added.

### Callback return values (phase 1)
Return a 2-tuple:
1. `timestamp_utc: float` (Unix timestamp seconds in UTC)
2. `local_offset_seconds: int` (host local UTC offset, in seconds, for that instant)

The return shape is identical for all callers. `time.time()` can reuse this callback and read only `timestamp_utc`.

### Monty-side conversion
1. If `tz=None`: Monty computes local civil time from `timestamp_utc + local_offset_seconds`, then constructs a **naive** datetime with those wall-clock components.
2. If `tz=timezone(offset)`: Monty uses `timestamp_utc` and applies the requested fixed offset on its side to produce an **aware** datetime.
3. `date.today()` calls the same callback and derives the date from the same local civil basis as `datetime.now(tz=None)`.

### Why this contract
1. Single source of truth for "now".
2. Reuses cleanly for future `time.time()` (same callback, Monty returns `timestamp_utc`).
3. Keeps host API simple and deterministic for tests.

## Runtime Data Model
Implement new heap types in `crates/monty/src/types/`:
1. `Date`
- Stored as proleptic Gregorian ordinal (days from epoch, single `i32`).
- Civil components `(year, month, day)` derived on demand for display/construction validation.
- Ordinal form makes arithmetic trivial (`date + timedelta` = ordinal addition) and comparison is integer comparison.
2. `DateTime`
- **Aware**: stored as UTC epoch microseconds (`i64`) + offset seconds (`i32`). Comparison/arithmetic operates on UTC microseconds directly.
- **Naive**: stored as civil epoch microseconds (`i64`) with **no UTC semantics** — the value represents wall-clock time with no zone. A sentinel value (e.g., `offset = None` or a separate enum variant) distinguishes naive from aware.
- This distinction is critical: naive and aware datetimes must never be compared or subtracted (CPython raises `TypeError`), and conflating them by storing both as "UTC microseconds" would mask bugs.
3. `TimeDelta`
- Stored as normalized `(days: i32, seconds: i32, microseconds: i32)` matching CPython's internal representation.
- Invariants: `0 <= seconds < 86400`, `0 <= microseconds < 1_000_000`.
- Bounds enforced: `-999999999 <= days <= 999999999` (matches CPython). `OverflowError` on violation.
4. `TimeZone`
- Stored as fixed offset seconds (`i32`) and optional display name (`Option<StringId>`).
- Offset range: `-86340 <= offset_seconds <= 86340` (matching CPython's `timedelta(hours=23, minutes=59)` limit). `ValueError` on violation.
- Offset must be an exact whole number of minutes (`offset_seconds % 60 == 0`) to match CPython `datetime.timezone` validation.

All four types are immutable and contain no heap references, so `is_gc_tracked()` returns `false`.

Representation must make arithmetic/comparison cheap and deterministic.

## Rust Library Choice
1. Use `speedate` for date/datetime/timedelta runtime storage and timestamp conversion.
2. Keep CPython-specific error semantics and formatting in Monty wrappers around `speedate` types.

## Core Implementation Tasks

### Task dependency order
Tasks 1-2 (wiring + type system) → Task 5 (OS enum) → Task 3 (behavior) → Task 4 (VM call-path) → Task 6 (arithmetic) → Task 7 (exceptions used throughout, but helpers should be added as needed during tasks 3-6).

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
1. **Refactor `call_type_method` return type**: currently returns `Result<Value, RunError>`, but `date.today()` and `datetime.now()` need to yield `AttrCallResult::OsCall`. Change the return type to `Result<AttrCallResult, RunError>` and update existing callers (`dict.fromkeys()`, `bytes.fromhex()`) to wrap their results in `AttrCallResult::Value(...)`.
2. Add dispatch arms for `Type::Date` and `Type::DateTime` classmethods.
3. Ensure ref-count safety in all early-return/error paths using `defer_drop!` or `HeapGuard`.

### 5) OS function enum and dispatch
1. Add `DateTimeNow` in `crates/monty/src/os.rs`.
2. Ensure `RunProgress::OsCall` stringification yields `datetime.now` in bindings.

### 6) Arithmetic integration
1. Update `crates/monty/src/value.rs` binary operation routing so `Value::Ref` subtraction can delegate to heap type ops (mirroring current `Ref + Ref` behavior in `py_add`), instead of only current LongInt-specialized handling.
2. Preserve existing numeric behavior and avoid regressions.

### 7) Exception parity
1. Use existing exception helper patterns in `crates/monty/src/exception_private.rs`.
2. Add dedicated helper constructors for repeated datetime errors.
3. Match CPython error classes/messages for:
- invalid constructor ranges
- aware vs naive mismatch
- invalid timezone offset ranges

## Native Python Binding Tasks
### 1) Public object model
Update `crates/monty/src/object.rs`:
1. Add `MontyObject` variants with explicit payloads:
- `MontyObject::Date { year: i32, month: u8, day: u8 }`
- `MontyObject::DateTime { year: i32, month: u8, day: u8, hour: u8, minute: u8, second: u8, microsecond: u32, offset_seconds: Option<i32> }` (None = naive)
- `MontyObject::TimeDelta { days: i32, seconds: i32, microseconds: i32 }`
- `MontyObject::TimeZone { offset_seconds: i32, name: Option<String> }`
2. Add serialization (JSON: use civil components for human readability) and `to_value`/`from_value` conversion paths.

### 2) Conversion layer
Update `crates/monty-python/src/convert.rs`:
1. `py_to_monty`:
- recognize Python `datetime.date`, `datetime.datetime`, `datetime.timedelta`, `datetime.timezone`
- convert to the `MontyObject` variants defined above (extract civil components from native Python objects)
2. `monty_to_py`:
- reconstruct native Python datetime objects from `MontyObject` variant payloads (not strings/tuples)

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

**Dependency note**: The datatest harness mock dispatcher (task 2) must be implemented before `# call-external` test cases (task 3) can run, since those tests need `DateTimeNow` to be handled deterministically.

### 1) Datatest harness (implement first)
1. Update `crates/monty/tests/datatest_runner.rs` OS dispatcher:
- handle `DateTimeNow` deterministically with a fixed fixture payload:
  - `timestamp_utc = 1700000000.0` (2023-11-14 22:13:20 UTC)
  - `local_offset_seconds = 0` (or another explicit fixed value for offset-focused cases)
2. Update `scripts/iter_test_methods.py` with CPython-side equivalent behavior for `# call-external` tests.

### 2) Rust unit/integration tests
1. Extend `crates/monty/tests/os_tests.rs`:
- verify `date.today()` and `datetime.now(...)` yield `OsCall` with `OsFunction::DateTimeNow`
- verify callback args/kwargs are empty and return payload shape is validated
2. Add arithmetic/unit tests for date/datetime/timedelta semantics and error cases.

### 3) Python test cases (core)
Add consolidated file `crates/monty/test_cases/datetime__core.py` with `# call-external` covering:
1. `date.today()` from virtual now.
2. `datetime.now(timezone.utc)` and non-zero fixed offsets.
3. date/datetime +/- timedelta operations.
4. datetime subtraction results.
5. aware vs naive exception exactness.
6. `timezone.utc` constant access.
7. naive/aware equality and inequality behavior exactness (`==`/`!=` do not raise).
8. timezone offset minute-granularity validation.

### 4) Python package tests
Add/extend `crates/monty-python/tests/test_os_calls.py`:
1. Snapshot of `MontySnapshot.function_name == 'datetime.now'`.
2. Resume with callback payload tuple `(timestamp_utc, local_offset_seconds)` and assert Monty output uses native datetime classes.
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
3. Callback payload validation enforces strict types/ranges.
4. No unbounded allocations from datetime operations — `timedelta` bounds (`-999999999 <= days <= 999999999`) enforced on all constructors and arithmetic results.
5. Timezone offset validation enforced (`-86340..=86340` seconds and whole-minute granularity).
6. Ref-count correctness verified on all error paths.

## Rollout Strategy
1. Land phase 1 as one feature-complete PR for fixed-offset + native Python bindings.
2. Keep JS behavior unchanged for now (explicitly unsupported OS calls in JS run loop remains).
3. Follow with phase 2 PR for `time.time()`, parsing/formatting, and broader stdlib parity.

## Phase 2 Preview (Not in this PR)
1. Implement `time.time()` using the same `datetime.now` callback contract (read `timestamp_utc` from callback payload).
2. Add ISO parse/format support (`speedate` integration candidate).
3. Add richer constructors (`fromtimestamp`, UTC variants).
4. Evaluate IANA timezone/DST support with clear host policy boundaries.
