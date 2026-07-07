# REST API Reference

The REST API provides remote control of the display. Disabled by default when `api.enabled: false`.

Default bind: `0.0.0.0:9110`

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check |
| `GET` | `/status` | Connection status, values, errors |
| `GET` | `/config` | Current config file |
| `POST` | `/reload` | Hot-reload config |
| `POST` | `/display/value` | Override a slot's value |
| `POST` | `/display/raw` | Push a full frame |
| `POST` | `/display/off` | Blank the display |
| `POST` | `/display/clear` | Clear all overrides |

### Live web preview

When `api.preview_enabled: true`, an extra set of routes is served under `/preview`:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/preview` | Self-contained HTML dashboard |
| `GET` | `/preview/frame` | Latest rendered frame as JSON |

#### GET /preview

Serves an embedded HTML page that polls `/preview/frame` and renders the display
as 7-segment digits with real RGB colours. No external dependencies — the page
is self-contained (CSS + JS inlined).

#### GET /preview/frame

Returns the latest rendered frame snapshot:

```json
{
    "generation": 1234,
    "profile": "PS120_DIGITAL",
    "leds": [
        [0, 200, 120],
        [0, 0, 0],
        ...
    ]
}
```

- `generation` — monotonically increasing counter; clients skip re-renders when
  it hasn't changed.
- `profile` — active profile name.
- `leds` — physical-order RGB colours, one per LED (length = `mask_size`).

Returns `503 Service Unavailable` until the first frame is rendered (device not
identified yet).

## GET /health

Returns `ok` on success.

```bash
curl localhost:9110/health
# → ok
```

## GET /status

Returns connection status, tile values, errors, and overrides.

```bash
curl localhost:9110/status | jq
```

Response:
```json
{
    "connected": true,
    "pm": 3,
    "sub": 9,
    "profile": "AX120_DIGITAL",
    "source": "prometheus",
    "frames_sent": 1234,
    "last_frame_ms_ago": 150,
    "values": {
        "cpu_temp": 57.0,
        "gpu_temp": 72.5
    },
    "errors": {},
    "overrides": ["main"],
    "raw_override": false,
    "last_error": null
}
```

## GET /config

Returns the current config file as JSON.

```bash
curl localhost:9110/config | jq
```

## POST /reload

Re-read the config file. Tiles and render settings apply immediately; source kind, URL, and bind address need a restart.

```bash
curl -XPOST localhost:9110/reload
# → {"ok":true,"message":"reloaded"}
```

## POST /display/value

Override a slot's value.

Request:
```json
{
    "slot": "gpu_temp",        // required: slot name
    "value": 72,               // required: numeric value
    "unit": "celsius",         // optional: unit marker
    "color": [255, 60, 60],    // optional: RGB color
    "ttl_seconds": 30          // optional: time-to-live (0 = permanent)
}
```

Response:
```json
{"ok": true, "message": "slot \"gpu_temp\" set to 72"}
```

## POST /display/raw

Push a full frame (must match profile's LED count exactly).

Request:
```json
{
    "colors": [               // required: exactly mask_size RGB triples
        [255, 0, 0],
        [0, 255, 0],
        ...
    ],
    "ttl_seconds": 10         // optional: time-to-live
}
```

Response:
```json
{"ok": true, "message": "raw frame set"}
```

Error (wrong size):
```json
{"error": "profile AX120_DIGITAL needs exactly 30 colors, got 2"}
```

## POST /display/off

Blank the display (all LEDs off).

Request:
```json
{
    "ttl_seconds": 60         // optional: time-to-live
}
```

Response:
```json
{"ok": true, "message": "display blanked"}
```

## POST /display/clear

Remove all overrides (slot and raw). Resume normal metric-driven display.

```bash
curl -XPOST localhost:9110/display/clear
# → {"ok": true, "message": "cleared all overrides"}
```

## Override precedence

1. **Raw override** (`/display/raw`) — takes full control of the display
2. **Slot override** (`/display/value`) — overrides individual slots
3. **Metrics** — normal tile values (fallback)

Overrides expire after `ttl_seconds` or when `/display/clear` is called.

## Example: dashboard script

```bash
#!/bin/bash
# Show GPU temp on the display for 30 seconds
TEMP=$(curl -s http://localhost:9090/api/v1/query\?query\='node_gpu_temp' | jq -r '.data.result[0].value[1]')
curl -XPOST localhost:9110/display/value \
  -H 'content-type: application/json' \
  -d "{\"slot\":\"gpu_temp\",\"value\":${TEMP},\"unit\":\"celsius\",\"ttl_seconds\":30}"
```