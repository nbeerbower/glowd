# glowd

Tiny self-hosted web controller for MagicHome / Zengge LED strip controllers
(the ones the "Magic Home" app talks to). No cloud, no accounts — it speaks the
controllers' local LAN protocol directly.

Single static binary (Rust, `tiny_http`), web UI embedded at compile time.

## Build & run

```sh
cargo build --release
./target/release/glowd            # listens on http://0.0.0.0:5578
./target/release/glowd --port 80  # or wherever
```

On startup glowd broadcasts a discovery probe and lists every controller it
finds; the **Rescan** button in the UI re-runs discovery.

## Run as a service (systemd)

```sh
sudo cp target/release/glowd /usr/local/bin/
sudo cp glowd.service /etc/systemd/system/
sudo systemctl enable --now glowd
```

## HTTP API

| Method | Path            | Body                                        | Notes                          |
| ------ | --------------- | ------------------------------------------- | ------------------------------ |
| GET    | `/`             | —                                           | web UI                         |
| GET    | `/api/devices`  | —                                           | known devices + live state     |
| GET    | `/api/effects`  | —                                           | list of effect names           |
| POST   | `/api/discover` | `{}`                                        | re-scan the LAN, returns devices |
| POST   | `/api/power`    | `{"ip": "...", "on": true}`                 |                                |
| POST   | `/api/color`    | `{"ip": "...", "r": 255, "g": 17, "b": 0}`  | switches to solid-color mode   |
| POST   | `/api/effect`   | `{"ip": "...", "name": "red_gradual", "speed": 50}` | speed 1–100 (fastest) |
| GET    | `/api/colors`   | —                                           | saved palette (hex strings)    |
| POST   | `/api/colors`   | `{"hex": "#ff8800"}`                        | save a color, returns palette  |
| POST   | `/api/colors/remove` | `{"hex": "#ff8800"}`                   | forget a color, returns palette |

Saved colors persist to `colors.json` in the state dir — `--state-dir DIR`,
`$GLOWD_STATE_DIR`, or `~/.local/state/glowd` by default (the systemd unit
uses `/var/lib/glowd` via `StateDirectory`).

## Protocol notes

MagicHome controllers speak an unencrypted protocol on the LAN:

- **Discovery** — UDP broadcast `HF-A11ASSISTHREAD` to port `48899`; each
  device replies `ip,mac,model`.
- **Control** — TCP port `5577`. Commands are a handful of bytes plus a
  checksum (sum of all bytes, truncated to 8 bits):
  - power: `71 23 0F` on / `71 24 0F` off
  - solid color: `31 R G B 00 F0 0F`
  - effect: `61 <mode> <speed> 0F` (mode `0x25`–`0x38`, speed `0x01` fastest–`0x1F` slowest)
  - state query: `81 8A 8B` → 14-byte reply (power at byte 2, mode at 3, RGB at 6–8)
