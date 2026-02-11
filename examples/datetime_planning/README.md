# Datetime Planning

This example uses Monty's phase-1 `datetime` support to compute:

1. Current local date (`date.today()`)
2. Local time in one hour (`datetime.now() + timedelta(hours=1)`)
3. Next Thursday at 3pm local time
4. First Monday of next month

The current-time source comes from Monty's OS callback (`datetime.now`) through `OSAccess`.

## Run

```bash
uv run python examples/datetime_planning/main.py
```
