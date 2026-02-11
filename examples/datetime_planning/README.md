# Datetime Planning

This example uses Monty's phase-1 `datetime` support to compute:

1. Current date in fixed-offset PST (`datetime.now(pst)`)
2. Time in one hour in fixed-offset PST
3. Next Thursday at 3pm in fixed-offset PST
4. First Monday of next month

Display format:
- Time values are printed in 12-hour format with AM/PM (fixed PST label).

The current-time source comes from Monty's OS callback (`datetime.now`) through `OSAccess`.

Note:
- This uses a fixed offset (`UTC-08:00`) named `PST`.
- DST transitions are not handled in phase-1 support.

## Run

```bash
uv run python examples/datetime_planning/main.py
```
