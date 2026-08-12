# Functionality Status

This table tracks the user-facing feature areas of the protocol-defined lens
feature set and their status in the stateless Linux CLI. "Automated" means
covered by a fake transport, parser, or domain test; "hardware" names behavior
exercised on the connected A068 lens. Hardware write verification means the
setting was written, read back, and restored; it does not claim that the
resulting focus or camera behavior was measured optically.

| Area | Functionality | Implementation | Verification | Notes |
|------|---------------|----------------|--------------|-------|
| Platform | Linux-only CLI and reusable Rust library | Done | Automated | Non-Linux builds fail explicitly. |
| Start | Device discovery and selection | Done | Automated + hardware | `devices`; USB serial preferred, port path fallback. |
| Start | Connection safety notice | Done | Help reviewed | Captured in detailed clap help instead of a welcome screen. |
| Start | Remember/suppress welcome screen | Deferred | N/A | Stateless CLI keeps no preferences. |
| Connection | Standalone connect and initial data load | Done | Automated + hardware | Descriptor plus settings blocks 0 and 1. |
| Connection | Camera-attached connect and attempted writes | Done | Automated | Result `0x83` is surfaced contextually. |
| Connection | Recovery-mode firmware prompt | Deferred | N/A | Reserve `tlc firmware recovery-update`; normal connected updates do not enter recovery flow. |
| Connection | Explicit remove/disconnect | Done | Automated + hardware | Every connected command sends `0xF9`, then closes. |
| Home | Product, model, mount, class, and firmware | Done | Hardware | `info` displays firmware as `MAJOR.MINOR`. |
| Home | Current settings summary | Done | Hardware | `info` includes every advertised setting. |
| Home | Save current settings | Done | Automated + hardware | A068 snapshot saved; a second save to the same path was refused. |
| Home | Load settings | Done | Automated + hardware write/read/restore | Prompt cancellation and confirmation tested; restored image matched the original byte for byte. |
| Home | Factory reset | Done | Automated + hardware reset/read/restore | Prompt cancellation and `--yes` tested; original snapshot was restored and verified afterward. |
| Focus ring | Focus/aperture ring function | Done | Automated; unavailable on A068 | Capability-gated enum; A068 rejects the setting as unsupported. |
| Focus ring | Forward/reverse/camera direction | Done | Automated + hardware write/read/restore | Forward, reverse, and camera-controlled values have been exercised on A068. |
| Focus ring | Linear/nonlinear response | Done | Automated + hardware write/read/restore | A068 was changed to linear, read back, and restored to nonlinear. |
| Focus ring | Focus rotation angle | Done | Automated + hardware write/read/restore | A068 was changed from 180 to 270 degrees, read back, and restored. |
| Focus ring | Aperture rotation angle | Done | Automated; unavailable on A068 | Degrees; descriptor range; requires aperture function. |
| Focus ring | M/A override sensitivity | Done | Automated; unavailable on A068 | Values 0 through 2. |
| Custom switch | Mode selection | Done | Automated; unavailable on A068 | A068 reports zero Custom Switch positions and rejects the command. |
| Custom switch | Position AF-limit windows | Done | Automated; unavailable on A068 | Indexed positions with strict near/far ordering. |
| Button | AF/MF variants | Done | Automated + hardware write/read/restore | A068 accepted and read back both press and hold assignments. |
| Button | Focus limiter variants | Done | Automated; unavailable on A068 | Press, hold, momentary-limit, and momentary-full values. |
| Button | Astro fixed/fine functions | Done | Automated; unavailable on A068 | Fixed press/hold and adjustable infinity lock. |
| Button | Focus preset and A-B focus | Done | Automated + hardware write/read/restore | Untimed and timed variants, speed, and duration were exercised on A068. |
| Button | Focus stopper and ring stopper | Done | Automated | Press and hold variants. |
| Button | Focus/aperture ring switching | Done | Automated + hardware write/read/restore | A068 accepted and read back both press and hold assignments. |
| Button | Linear/nonlinear switching | Done | Automated | Press and hold variants. |
| Button | Focus hold / camera-assigned functions | Done | Automated + hardware write/read/restore | A068 focus hold was assigned and read back; both protocol IDs remain automated. |
| Button | VC switching | Done | Automated | Exposed when its capability bit is advertised. |
| Button | Timed iris preset / timed A-B iris | Done | Automated | Protocol-defined values 24 and 25 use separate iris duration and delay fields when advertised. |
| Button | Focus timelapse counts | Done | Automated | Move 0 normalizes to 1; tally reset follows count writes. |
| Button | Shared-byte conflict protection | Done | Automated | Conflicting speed/move-count interpretations are rejected. |
| Focus calibration | Focus-point correction | Done | Automated | Signed value bounded by descriptor half-range. |
| Firmware | Installed firmware version | Done | Hardware | Network-free local descriptor value. |
| Firmware | Latest-version network check | Done | Automated + hardware | A068 reported installed `01`, available `03` before updating, then installed and available `03` afterward; both checks disconnected normally. |
| Firmware | Normal network update | Done | Automated + hardware (ordinary path) | A068 updated from `01.00` to `03.00` using the ordinary `(device 0, area 0)` path. Exact-serial staged rediscovery is implemented but not hardware-tested. |
| Firmware | Local update and safe-mode recovery | Deferred | N/A | Future `firmware recovery-update`; raw maintenance images need independent model, target, size, and hash controls. |
| Online | Function list, browser links, privacy, download site | Deferred | N/A | GUI/network navigation is outside the CLI v1 scope. |
| Utility | CLI software version | Done | Help reviewed | `tlc --version` uses Cargo package metadata. |
| Runtime | Immediate acknowledged writes | Done | Automated + hardware | Ring, button, restore, and reset writes were acknowledged and re-read on A068. |
| Runtime | Verbose command and raw frame diagnostics | Done | Automated + hardware | Top-level `-v` logged operations; `-vv` displayed complete TX/RX frames from A068. |
| Runtime | Hot unplug and lens communication errors | Done | Automated | Reported as runtime errors; no persistent monitor is needed. |
| Runtime | Close protection | Done | Automated + hardware | Successful and rejected A068 commands sent the explicit disconnect where a session was established. |
| Appearance | Light/dark theme and accent colors | Deferred | N/A | Not applicable to a plain-text CLI. |

## Remaining Work

- Recovery update support remains deferred until its CLI safety and hardware
  verification procedure are specified.

## Hardware Test Record

Tested on 2026-08-07 with an A068 (17-50mm F/4.0 Di III VXD, Sony E,
firmware 01.00) connected as `/dev/ttyUSB0`.

- Discovery, automatic selection, serial selection, and port selection passed.
- Identity, capabilities, current settings, `-v`, `-vv`, and explicit
  disconnect passed.
- `firmware check` fetched the A068SE manifest over HTTPS, reported installed
  `01` and available `03`, and completed the explicit disconnect.
- Backup creation, existing-file refusal, restore confirmation, restore,
  factory-reset confirmation, and factory reset passed.
- Every setting advertised by this A068 was written and read back: all three
  ring directions, both ring responses, focus rotation angle, all advertised
  Focus Set Button assignments, preset speed, and timed-focus duration.
- The original and final snapshots had the same SHA-256 digest:
  `7010b176e0cadd8851fcdd5f7ddbed88b6d0eaefff5cb6497c25934c07cd46b1`.
- Custom Switch, aperture-ring, focus-calibration, delay, and the remaining
  button functions cannot be hardware-tested with A068 because it does not
  advertise them. Camera-attached, recovery, and hot-unplug paths also remain
  automated-only because those states were not available during this run.

Firmware update tested on 2026-08-12 with the same A068 lens:

- `firmware update --force` downloaded and validated `A068SE_0300.tfwf`, then
  transferred the ordinary `(device 0, area 0)` image. All 511 data blocks were
  acknowledged and the EOT handshake completed.
- A subsequent `firmware check` reported installed and available version `03`,
  with status `up to date`.
- A subsequent `tlc info` reported firmware `03.00`, a standalone connection,
  readable settings, and a clean explicit disconnect.
- Staged area-2 re-enumeration, recovery updates, and transfer failure paths
  have not been hardware-tested.
