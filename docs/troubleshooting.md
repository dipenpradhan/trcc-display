# Troubleshooting

## The display doesn't respond

### Check USB detection

```bash
# Is the device visible?
trcc-display detect

# Expected output:
# found 1 device(s):
#   bus 001 device 042: 0416:8001
```

If no devices found:
1. Check the USB connection (re-plug)
2. Verify the cooler is powered
3. Check `lsusb | grep 0416` for the raw device list

### Permission denied

```
error: permission denied opening 0416:8001
```

**Fix:** Install the udev rule:
```bash
sudo cp packaging/udev/99-thermalright-trcc.rules /etc/udev/rules.d/
sudo udevadm control --reload
sudo udevadm trigger
# Re-plug the USB device
```

Or run as root (not recommended for production):
```bash
sudo trcc-display run
```

### Handshake fails

```
handshake failed
```

The firmware only answers once per power cycle. The driver caches the result in `state/probe_cache.json`. If the cache is stale:

```bash
rm state/probe_cache.json
# Power cycle the cooler (unplug, wait 5s, replug)
trcc-display probe
```

## Wrong LEDs light up

### LEDs don't match the digits

Your cooler may have a different wire mapping. Use the test pattern:

```bash
trcc-display test-pattern --mode walk
```

Write down which physical LED lights up for each index, then create a custom profile with a `remap` table. See [Device Profiles](./profiles.md).

### Display shows garbage

Check that your profile matches the cooler model:

```bash
trcc-display probe
# → PM=3 SUB=9 → AX120_DIGITAL
```

If the PM byte doesn't match any profile, you may have a different model. Add a new profile or use `force`:

```json
"profile": { "force": "pa120" }
```

## No data from Prometheus

### Query returns no series

```json
{ "errors": { "cpu_temp": "no data (query matched nothing)" } }
```

Check the query directly:
```bash
curl "http://localhost:9090/api/v1/query?query=YOUR_QUERY" | jq
```

### Timeout

Increase the timeout in config:
```json
"prometheus": { "timeout_seconds": 10 }
```

## lm-sensors not working

### sensors command not found

```bash
sudo apt install lm-sensors
sensors-detect  # run if needed
```

### Wrong chip/feature path

```bash
sensors -j | jq '.keys()'
```

Use the exact path from the JSON output:
```json
{ "query": "k10temp-pci-00c3/Tctl/temp1_input" }
```

## Docker issues

### Device not passed through

Ensure the `docker-compose.yml` maps the USB bus:
```yaml
devices:
  - /dev/bus/usb:/dev/bus/usb
```

### Permission errors in container

The container runs as non-root. Ensure the udev rule is applied on the host:
```bash
sudo cp packaging/udev/99-thermalright-trcc.rules /etc/udev/rules.d/
sudo udevadm control --reload
```

## General debugging

### Enable verbose logging

```bash
# Debug level
trcc-display -v run

# Trace level (full debug)
trcc-display -vv run

# Or with RUST_LOG
RUST_LOG=trcc_display=debug trcc-display run
```

### Check the REST API

```bash
# Status
curl localhost:9110/status | jq

# Override to test
curl -XPOST localhost:9110/display/value \
  -H 'content-type: application/json' \
  -d '{"slot":"main","value":42,"unit":"celsius","ttl_seconds":5}'
```

### Test with render command

```bash
# Bypass all metric sources, just render a value
trcc-display render --slot main --value 88 --unit celsius
```