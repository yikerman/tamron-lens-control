# Tamron Lens Control

`tlc` is a Linux-only command-line utility for viewing and changing settings on compatible Tamron lenses. It aims to reproduce what the official Tamron Lens Utility has to offer on Linux.

## Disclaimer

- **No affiliation** `tamron-lens-control` is an independent, community-driven project and is not affiliated with, endorsed by, or sponsored by Tamron Co., Ltd. "Tamron" is a trademark of its respective owner, used here only to identify compatible products.
- **No warranty** `tamron-lens-control` is licensed under *GNU GPL v3 or later* and comes with absolute zero warranty. While I do hope it provides help, on using this software, you accept that your lens may brick, bounce away, shoot around the room like a frightened sparrow or leave the Earth and the Solar System. See `LICENSE`.

## Install

Build and install from this repository with a recent Rust toolchain:

```bash
cargo install --path .
```

You can also find a portable binary in GitHub Release.

Connect the lens directly over USB, then confirm that `tlc` can see it:

```bash
tlc devices
tlc info
```

When several lenses are connected, select one by the serial number or port shown by `tlc devices`:

```bash
tlc --device SERIAL info
tlc --device /dev/ttyUSB0 info
```

## Use

```bash
## EXAMPLES
## Each command connects to the lens, performs one action, and disconnects.
## Note that a lens may support only a subset of them.

# View focus ring settings
tlc ring get

# Reverse the focus ring direction
tlc ring set direction reverse

# View Focus Set Button and Custom Switch assignments
tlc button get

# Assign Focus Preset to the Focus Set Button
tlc button set focus function focus-preset

# Fine-tune autofocus accuracy
tlc focus-calibration set 2

## some more.. see tlc --help
```

Run `tlc --help` or add `--help` after any command for available settings, accepted values, and lens-specific requirements.

Place `-v` before the command to show each operation sent to the lens, and `-vv` to also print all raw transmitted and received bytes in hexadecimal.

## Backups

Back up all current settings before making extensive changes:

```bash
tlc settings save my-lens.tlc
tlc settings load my-lens.tlc
```

Backup files can be restored only to the same lens model.

## Firmware

Check the firmware advertised for the connected lens without changing it:

```bash
tlc firmware check
```

Before updating, save the current settings and connect the lens directly over
USB. Firmware updates require the lens to expose a USB serial number so tlc can
identify the same lens if it re-enumerates during the transfer.

```bash
tlc settings save before-firmware.tlc
tlc firmware update
```

The update command shows the connected model, mount, installed firmware, and
advertised firmware before asking for confirmation. Use `--yes` to skip the
prompt or `--force` to reinstall an advertised version that matches the
installed display version. Keep power and USB connected until the command
finishes. Afterward, reconnect the lens and run `tlc info` to verify it.

Recovery-mode and local raw-image firmware installation are not exposed.

## Linux Driver Setup

If the lens appears in `lsusb` but not in `tlc devices`, Linux may need to be
told to use the `cp210x` USB serial driver for it.

### One-Time Setup

Use these commands to test the driver setup immediately:

```bash
sudo modprobe cp210x
echo 2cd1 0002 | sudo tee /sys/bus/usb-serial/drivers/cp210x/new_id
echo 2cd1 0005 | sudo tee /sys/bus/usb-serial/drivers/cp210x/new_id
```

Reconnect the lens and check for its serial port:

```bash
tlc devices
```

If `/dev/ttyUSB0` exists but cannot be opened, grant your current user temporary access:

```bash
sudo setfacl -m u:"$USER":rw /dev/ttyUSB0
```

### Persistent Setup

Create one udev rule that registers the Tamron USB IDs and grants the active desktop user access whenever a compatible lens is connected:

```bash
sudoedit /etc/udev/rules.d/70-tamron-lens.rules
```

Add these lines:

```udev
ACTION=="add", SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_interface", DRIVER=="", ATTRS{idVendor}=="2cd1", ATTRS{idProduct}=="0002", RUN+="/bin/sh -c '/sbin/modprobe cp210x && echo 2cd1 0002 > /sys/bus/usb-serial/drivers/cp210x/new_id'"
ACTION=="add", SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_interface", DRIVER=="", ATTRS{idVendor}=="2cd1", ATTRS{idProduct}=="0005", RUN+="/bin/sh -c '/sbin/modprobe cp210x && echo 2cd1 0005 > /sys/bus/usb-serial/drivers/cp210x/new_id'"
SUBSYSTEM=="tty", ATTRS{idVendor}=="2cd1", ATTRS{idProduct}=="0002", TAG+="uaccess"
SUBSYSTEM=="tty", ATTRS{idVendor}=="2cd1", ATTRS{idProduct}=="0005", TAG+="uaccess"
```

Reload the rules, then unplug and reconnect the lens:

```bash
sudo udevadm control --reload-rules
# reconnect
tlc devices && tlc info
getfacl /dev/ttyUSB0
```

For SSH or a headless system, `uaccess` may not apply. Check the device group:

```bash
stat -c '%G' /dev/ttyUSB0
```

If it reports `dialout`, add your user to that group and log out completely:

```bash
sudo usermod -aG dialout "$USER"
```

Note that group membership grants access to every device owned by `dialout`, not only the lens. The question lies more in why are you configuring lens through a headless system.

## Safety and Scope

- Keep the lens connected until a command finishes.
- Review a setting with its `get` command before changing it.
- Use `settings save` before loading another backup or performing a reset.
- Back up settings before firmware updates and do not interrupt power or USB.
- Recovery-mode and local raw-image firmware installation remain unsupported.
