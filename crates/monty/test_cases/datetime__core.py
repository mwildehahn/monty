# call-external
import datetime

# === now/today from deterministic OS callback ===
today = datetime.date.today()
assert str(today) == '2023-11-14', f'date.today() should use deterministic local date from callback, got {today!s}'

now_local = datetime.datetime.now()
assert str(now_local) == '2023-11-14 22:13:20', (
    f'datetime.now() should use deterministic local wall clock, got {now_local!s}'
)

now_utc = datetime.datetime.now(datetime.timezone.utc)
assert str(now_utc) == '2023-11-14 22:13:20+00:00', 'datetime.now(timezone.utc) should be aware UTC'

plus_two = datetime.timezone(datetime.timedelta(hours=2))
now_plus_two = datetime.datetime.now(plus_two)
assert str(now_plus_two) == '2023-11-15 00:13:20+02:00', 'datetime.now() with fixed offset should adjust civil time'

# === repr/str parity ===
assert repr(datetime.date(2024, 1, 15)) == 'datetime.date(2024, 1, 15)', 'date repr should match CPython'
assert str(datetime.date(2024, 1, 15)) == '2024-01-15', 'date str should match CPython'
assert repr(datetime.datetime(2024, 1, 15, 10, 30)) == 'datetime.datetime(2024, 1, 15, 10, 30)', (
    'datetime repr should omit trailing zero fields'
)
assert str(datetime.datetime(2024, 1, 15, 10, 30)) == '2024-01-15 10:30:00', 'datetime str should include seconds'
assert repr(datetime.timedelta(days=1, seconds=3600)) == 'datetime.timedelta(days=1, seconds=3600)', (
    'timedelta repr should match CPython'
)
assert str(datetime.timedelta(days=1, seconds=3600)) == '1 day, 1:00:00', 'timedelta str should match CPython'
assert repr(datetime.timezone.utc) == 'datetime.timezone.utc', 'timezone.utc repr should match CPython'
assert (
    repr(datetime.timezone(datetime.timedelta(seconds=3600))) == 'datetime.timezone(datetime.timedelta(seconds=3600))'
), 'timezone repr should match CPython'
assert str(datetime.timezone(datetime.timedelta(seconds=61))) == 'UTC+00:01:01', (
    'timezone str should include second-level offsets'
)
assert (
    repr(datetime.timezone(datetime.timedelta(seconds=-1)))
    == 'datetime.timezone(datetime.timedelta(days=-1, seconds=86399))'
), 'timezone repr should normalize negative second offsets like CPython'
assert (
    repr(datetime.timezone(datetime.timedelta(hours=1), 'A'))
    == "datetime.timezone(datetime.timedelta(seconds=3600), 'A')"
), 'timezone repr should use Python string quoting for custom names'
assert str(datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone(datetime.timedelta(seconds=61)))) == (
    '2024-01-01 00:00:00+00:01:01'
), 'datetime str should include second-level offsets'
assert repr(datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone(datetime.timedelta(seconds=-1)))) == (
    'datetime.datetime(2024, 1, 1, 0, 0, tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=86399)))'
), 'datetime repr should use normalized negative timezone offsets'

# === arithmetic ===
assert datetime.date(2024, 1, 10) + datetime.timedelta(days=5) == datetime.date(2024, 1, 15), (
    'date + timedelta should add days'
)
assert datetime.date(2024, 1, 10) - datetime.timedelta(days=5) == datetime.date(2024, 1, 5), (
    'date - timedelta should subtract days'
)
assert datetime.date(2024, 1, 10) - datetime.date(2024, 1, 1) == datetime.timedelta(days=9), (
    'date - date should return timedelta'
)

base_dt = datetime.datetime(2024, 1, 10, 12, 0, 0)
assert base_dt + datetime.timedelta(hours=2) == datetime.datetime(2024, 1, 10, 14, 0, 0), (
    'datetime + timedelta should add duration'
)
assert base_dt - datetime.timedelta(hours=2) == datetime.datetime(2024, 1, 10, 10, 0, 0), (
    'datetime - timedelta should subtract duration'
)
assert datetime.datetime(2024, 1, 10, 12, 0, 0) - datetime.datetime(2024, 1, 10, 11, 0, 0) == datetime.timedelta(
    hours=1
), 'datetime - datetime should return timedelta'

assert datetime.timedelta(days=1, seconds=10) + datetime.timedelta(seconds=5) == datetime.timedelta(
    days=1, seconds=15
), 'timedelta + timedelta should add'
assert datetime.timedelta(days=1, seconds=10) - datetime.timedelta(seconds=5) == datetime.timedelta(
    days=1, seconds=5
), 'timedelta - timedelta should subtract'
assert -datetime.timedelta(days=1, seconds=30) == datetime.timedelta(days=-2, seconds=86370), (
    'unary -timedelta should normalize like CPython'
)
assert datetime.timedelta(hours=1, minutes=30).total_seconds() == 5400.0, (
    'timedelta.total_seconds() should match CPython'
)

# === aware/naive comparison and subtraction rules ===
aware = datetime.datetime(2024, 1, 1, 12, 0, 0, tzinfo=datetime.timezone.utc)
naive = datetime.datetime(2024, 1, 1, 12, 0, 0)

assert (aware == naive) is False, 'aware == naive should be False, not an exception'
assert (aware != naive) is True, 'aware != naive should be True, not an exception'

try:
    aware < naive
    assert False, 'aware < naive should raise TypeError'
except TypeError as e:
    assert str(e) == "can't compare offset-naive and offset-aware datetimes", (
        'aware/naive ordering message should match CPython'
    )

try:
    aware - naive
    assert False, 'aware - naive should raise TypeError'
except TypeError as e:
    assert str(e) == "can't subtract offset-naive and offset-aware datetimes", (
        'aware/naive subtraction message should match CPython'
    )

# === timezone validations and constant ===
assert datetime.timezone.utc == datetime.timezone(datetime.timedelta(0)), (
    'timezone.utc should equal zero offset timezone'
)
assert datetime.timezone(datetime.timedelta(hours=1), 'A') == datetime.timezone(datetime.timedelta(hours=1), 'B'), (
    'timezone equality should depend on offset, not name'
)
assert hash(datetime.timezone(datetime.timedelta(hours=1), 'A')) == hash(
    datetime.timezone(datetime.timedelta(hours=1), 'B')
), 'timezone hash should depend on offset, not name'
assert repr(datetime.timezone(datetime.timedelta(seconds=1))) == 'datetime.timezone(datetime.timedelta(seconds=1))', (
    'timezone should allow second-level fixed offsets'
)

try:
    datetime.timezone(datetime.timedelta(hours=24))
    assert False, 'timezone offset at 24 hours should raise ValueError'
except ValueError as e:
    assert str(e) == (
        'offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), '
        'not datetime.timedelta(days=1)'
    ), 'timezone range validation message should match CPython'

# === arithmetic overflow errors ===
try:
    datetime.date(1, 1, 1) - datetime.timedelta(days=1)
    assert False, 'date underflow should raise OverflowError'
except OverflowError as e:
    assert str(e) == 'date value out of range', 'date underflow should match CPython overflow message'

try:
    datetime.datetime(9999, 12, 31, 23, 59, 59, 999999) + datetime.timedelta(microseconds=1)
    assert False, 'datetime overflow should raise OverflowError'
except OverflowError as e:
    assert str(e) == 'date value out of range', 'datetime overflow should match CPython overflow message'

try:
    datetime.timedelta(days=999999999) + datetime.timedelta(days=1)
    assert False, 'timedelta addition overflow should raise OverflowError'
except OverflowError as e:
    assert str(e) == 'days=1000000000; must have magnitude <= 999999999', (
        'timedelta overflow should report the overflowing days value'
    )
