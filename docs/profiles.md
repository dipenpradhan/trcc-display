# Device Profiles

Profiles define the LED geometry for each cooler model. Adding a new cooler requires only a JSON file — no Rust changes.

## How profiles work

```
profile.json  ──▶  LED geometry (slots, digits, units, indicators)
                        │
tile config  ──▶  slot assignment (what to display where)
```

## Profile schema

```jsonc
{
    "name": "COOLER_MODEL",           // Unique name (e.g. "AX120_DIGITAL")
    "style": "cooler",                // Short name for lookup (e.g. "ax120")
    "pm_bytes": [1, 2, 3],           // Handshake PM values that select this profile
    "mask_size": 30,                 // Total LED count

    "remap": null,                   // Wire remap table (null = identity)
    "always_on": [0, 1],             // LEDs always lit (static decoration)

    "indicators": {                  // Named indicator LED groups
        "cpu": [2, 3],
        "gpu": [4, 5]
    },

    "slots": {                       // Named digit fields
        "main": {
            "digits": [              // 7-segment digits, MSB first
                [0, 1, 2, 3, 4, 5, 6],  // Each: [a, b, c, d, e, f, g]
                [7, 8, 9, 10, 11, 12, 13],
                [14, 15, 16, 17, 18, 19, 20]
            ],
            "partial": null,        // Leading '1' for hundreds (optional)
            "unit": {               // Unit marker LEDs
                "celsius": 21,
                "fahrenheit": 22,
                "percent": 23
            },
            "on": []                // Extra always-on LEDs for this slot
        }
    }
}
```

## Adding a new cooler

### 1. Map the LEDs

```bash
# Walk each LED one at a time
trcc-display test-pattern --mode walk --delay_ms 500

# Light all LEDs at once
trcc-display test-pattern --mode all
```

Write down which physical LED index corresponds to which segment of each digit.

### 2. Build the profile

```json
{
    "name": "MY_COOLER",
    "style": "mycooler",
    "pm_bytes": [42],
    "mask_size": 30,
    "remap": null,
    "always_on": [0],
    "indicators": {},
    "slots": {
        "main": {
            "digits": [
                [9, 10, 11, 12, 13, 14, 15],
                [16, 17, 18, 19, 20, 21, 22],
                [23, 24, 25, 26, 27, 28, 29]
            ],
            "unit": { "celsius": 6, "percent": 8 }
        }
    }
}
```

### 3. Place the file

Put the JSON in your `profile.dir` (e.g., `config/profiles/mycooler.json`).

### 4. Test

```bash
# Verify the profile loads
trcc-display --config config.json render --slot main --value 57 --unit celsius
```

## 7-segment layout

Each digit has 7 LEDs in this order:

```
     aaa (LED 0)
    f   b (LED 5, LED 1)
    f   b
     ggg (LED 6)
    e   c (LED 4, LED 2)
    e   c
     ddd (LED 3)
```

| Index | Segment | Description |
|-------|---------|-------------|
| 0 | a | Top horizontal |
| 1 | b | Upper-right vertical |
| 2 | c | Lower-right vertical |
| 3 | d | Bottom horizontal |
| 4 | e | Lower-left vertical |
| 5 | f | Upper-left vertical |
| 6 | g | Middle horizontal |

## Wire remapping

Some coolers (PA120, PS120) have non-trivial physical wiring. The `remap` table maps logical LED indices to physical wire positions:

```json
"remap": [1, 0, 3, 2, ...]  // physical[i] = logical[remap[i]]
```

If your cooler's LEDs don't match the profile, you may need to create a custom remap table. Use `test-pattern --mode walk` to verify.

## Partial digits (hundreds)

For 2-digit slots that need to show 0–199, the `partial` field defines two extra LEDs for the leading `1`:

```json
{
    "digits": [
        [31, 32, 33, 34, 35, 36, 37],  // tens digit
        [38, 39, 40, 41, 42, 43, 44]    // ones digit
    ],
    "partial": [80, 81]  // leading '1' for 100–199
}
```

Values 0–99 show normally. Values 100–199 light the partial LEDs + show 00–99.

## Multiple slots

Coolers with multiple fields (PA120, PS120) define multiple slots:

```json
{
    "slots": {
        "cpu_temp": { "digits": [...] },
        "cpu_use": { "digits": [...], "partial": [...] },
        "gpu_temp": { "digits": [...] },
        "gpu_use": { "digits": [...], "partial": [...] }
    }
}
```

Each slot is independent. Tiles in your config target slots by name.

## Validation

Profiles are validated on load. The driver refuses to start with invalid geometry:

- All LED indices must be `< mask_size`
- Each digit must have exactly 7 LEDs
- The remap table length must equal `mask_size`
- Slots must be non-empty

If validation fails, the error message tells you exactly which field is wrong.