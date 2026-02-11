"""Datetime planning example using Monty's phase-1 datetime support.

This script runs datetime calculations inside the Monty sandbox and prints:
- current local date (`date.today()`)
- local time in one hour (`datetime.now() + timedelta(hours=1)`)
- next Thursday at 3pm local time
- first Monday of next month
"""

from __future__ import annotations

from typing import cast

import pydantic_monty
from pydantic_monty import OSAccess

MONTY_CODE = """
import datetime


def parse_date(iso_date):
    year = int(iso_date[0:4])
    month = int(iso_date[5:7])
    day = int(iso_date[8:10])
    return year, month, day


def parse_time(iso_datetime):
    hour = int(iso_datetime[11:13])
    minute = int(iso_datetime[14:16])
    second = int(iso_datetime[17:19])
    return hour, minute, second


def weekday_monday_zero(year, month, day):
    # Sakamoto algorithm: 0=Sunday. Convert to Monday=0.
    offsets = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4]
    adjusted_year = year
    if month < 3:
        adjusted_year = adjusted_year - 1
    weekday_sunday_zero = (
        adjusted_year
        + adjusted_year // 4
        - adjusted_year // 100
        + adjusted_year // 400
        + offsets[month - 1]
        + day
    ) % 7
    return (weekday_sunday_zero + 6) % 7


today = datetime.date.today()
now_local = datetime.datetime.now()
in_one_hour = now_local + datetime.timedelta(hours=1)

today_iso = str(today)
now_iso = str(now_local)

year, month, day = parse_date(today_iso)
current_hour, _current_minute, _current_second = parse_time(now_iso)

today_weekday = weekday_monday_zero(year, month, day)
# Monday=0, Thursday=3
next_thursday_days = (3 - today_weekday + 7) % 7
if next_thursday_days == 0 and current_hour >= 15:
    next_thursday_days = 7

next_thursday_date = today + datetime.timedelta(days=next_thursday_days)
next_thursday_iso = str(next_thursday_date)
next_year, next_month, next_day = parse_date(next_thursday_iso)
next_thursday_3pm = datetime.datetime(next_year, next_month, next_day, 15, 0, 0)

if month == 12:
    first_year = year + 1
    first_month = 1
else:
    first_year = year
    first_month = month + 1

first_of_next_month = datetime.date(first_year, first_month, 1)
first_month_weekday = weekday_monday_zero(first_year, first_month, 1)
first_monday_offset = (0 - first_month_weekday + 7) % 7
first_monday_next_month = first_of_next_month + datetime.timedelta(days=first_monday_offset)

{
    'today': str(today),
    'in_one_hour': str(in_one_hour),
    'next_thursday_3pm': str(next_thursday_3pm),
    'first_monday_next_month': str(first_monday_next_month),
}
"""


def main() -> None:
    """Execute datetime calculations in Monty and print the computed schedule."""
    runner = pydantic_monty.Monty(MONTY_CODE, script_name='datetime_planning.py')
    result_any = runner.run(os=OSAccess())
    result = cast(dict[str, str], result_any)

    print('Datetime planning from Monty:')
    print(f'today: {result["today"]}')
    print(f'in_one_hour: {result["in_one_hour"]}')
    print(f'next_thursday_3pm: {result["next_thursday_3pm"]}')
    print(f'first_monday_next_month: {result["first_monday_next_month"]}')


if __name__ == '__main__':
    main()
