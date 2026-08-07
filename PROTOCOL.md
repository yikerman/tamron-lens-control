# Tamron Lens USB Serial Protocol - Clean Functional Specification

This document specifies the externally reproducible behavior of the USB
serial interface used to configure and update a compatible lens. It covers
wire bytes, response handling, timing, and the storage fields consumed or
changed by the host.

Names in this document are descriptive only. Numeric values and behavior are
normative. Values and paths without externally exercised behavior are not
assigned semantics.

Unless otherwise stated, multi-byte integer fields are little-endian.

---

## 1. Transport

### 1.1 Serial link

| Property | Command mode | Firmware-transfer mode |
|----------|--------------|------------------------|
| Media | USB CDC virtual serial port | Same port |
| Baud rate | 19200 | 3000000 |
| Data bits | 8 | 8 |
| Parity | None | None |
| Stop bits | 1 | 1 |
| Flow control | None | None |

The host accepts either of these USB identifiers:

| VID | PID |
|-----|-----|
| `2CD1` | `0002` |
| `2CD1` | `0005` |

Port discovery is outside the wire protocol.

### 1.2 Conversation rules

- In command mode the host sends one request and waits for its response before
  sending another request.
- The ordinary response wait is 500 ms. A timeout fails the operation; the
  host does not automatically retransmit it.
- A command response must begin at byte zero of the accumulated receive
  buffer. Prefix bytes are not safely accepted and must not be emitted.
- Firmware data frames use their own fixed-length format and have no trailing
  delimiter. Firmware control bytes are unframed single bytes.

---

## 2. Command Frames

### 2.1 Layout

```text
+------+------+------+----------+--------+------+---------+--------+--------+------+
| Start| Seq  | Dest | Reserved | Length | Op   | Data[N] | CRC Lo | CRC Hi | End  |
| 0x0F | 1 B  | 0x00 | 0x00     | 2 B LE | 1 B  | N bytes | 1 B    | 1 B    | 0xF0 |
+------+------+------+----------+--------+------+---------+--------+--------+------+
```

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 1 | Start | `0x0F` |
| 1 | 1 | Sequence | Host request counter; see below |
| 2 | 1 | Destination | `0x00` for every emitted operation |
| 3 | 1 | Reserved | `0x00` |
| 4 | 2 | Length | `N + 1`, including the Op byte |
| 6 | 1 | Op | Operation code |
| 7 | N | Data | Operation-specific bytes |
| `7 + N` | 2 | CRC | CRC-16, low byte first |
| `9 + N` | 1 | End | `0xF0` |

Total frame length is `N + 10` bytes.

The sequence state starts at zero, but zero is never transmitted. Before each
request it is incremented:

```text
01, 02, ..., FE, FF, 01, 02, ...
```

Transport teardown resets the state to zero, making the next request sequence
`0x01`. Sending the disconnect operation alone does not reset the counter.
A response sequence of zero or a value that differs from the current request is
ignored as out of sync.

### 2.2 Command-frame CRC

- Polynomial: `0x1021`.
- Initial value: `0x0000`.
- No reflection and no final XOR.
- Input: every byte from Start through the final Data byte.
- Storage: little-endian, low byte then high byte.

Equivalent byte update:

```text
crc = CRC16_TABLE[((crc >> 8) ^ byte) & 0xFF] ^ ((crc << 8) & 0xFFFF)
```

The table is the ordinary non-reflected `0x1021` table and may be generated
at run time.

### 2.3 Response acceptance

For a response beginning at receive-buffer offset zero, the host applies these
checks in this order:

1. Start `0x0F` and enough bytes to locate the footer from Length.
2. CRC equality.
3. If Op is `0xFF`, classify the frame as a communication-error response
   immediately. Sequence, destination, requested Op, and End are not checked
   for this case.
4. Sequence equality with the current nonzero request sequence.
5. Op equality with the request Op.
6. Destination equality with the request destination.
7. End equality with `0xF0`.

A CRC, Op, destination, or End failure completes the receive wait with an
invalid response. A sequence mismatch is discarded. There is no automatic
retry for either result.

### 2.4 Response data

The response Data length is `Length - 1`, beginning at frame offset 7.
For the operations that return a result byte, Data byte 0 is the result.

Memory-read responses are consumed as:

```text
[result][metadata byte 0][metadata byte 1][metadata byte 2][metadata byte 3][data...]
```

The four metadata bytes are skipped without validation. Requested memory
begins at response Data byte 5.

---

## 3. Emitted Operations

Only the following command operations are emitted or specially accepted:

| Operation | Op | Request Data |
|-----------|----|--------------|
| Read memory | `0xF4` | `[0x00, region, block, offset, size]` |
| Write memory | `0xF5` | `[0x00, region, block, offset, size] + payload` |
| Connect | `0xF8` | `[0x00]` |
| Disconnect notification | `0xF9` | Empty |
| Begin firmware transfer | `0xFD` | `[device, area]` |
| Communication-error response | `0xFF` | Response only |

No behavior is assigned here to other numeric operation values.

### 3.1 Memory selectors

The first byte of a memory operation selects its action:

| Value | Use |
|-------|-----|
| `0x00` | Ordinary read or write |
| `0x01` | Factory initialization when used with Op `0xF5` |

The region byte is:

| Value | Region | Host-consumed block size |
|-------|--------|--------------------------|
| `0x00` | Descriptor memory | 256 bytes |
| `0x01` | Settings memory | 256 bytes per block |

`block` is the page number and `offset` is the byte position within the
page. For reads, `size = 0` requests the 256-byte block. For ordinary
writes, `size` is 1 or 2. Settings restore is a special case in which
`size = 0` but a payload is still appended; see Section 5.3.

---

## 4. Result Handling

For memory and settings operations, the host applies one rule to Data byte 0:

| Result range | Host behavior |
|--------------|---------------|
| `0x00..0x7F` | Success |
| `0x80..0xFF` | Failure |

Values within each range are not otherwise distinguished, except that
`0x83` is surfaced to the user as an attached-camera or unsupported-
operation condition. No additional meanings are required for compatibility.

Op `0xFF` is separate from a result byte. A CRC-valid frame carrying that Op
releases the response wait as described in Section 2.3, but does not trigger
an automatic retransmission.

---

## 5. Command Sequences

### 5.1 Connection

1. Open an accepted USB serial port at 19200 baud.
2. Send Op `0xF8` with Data `[0x00]`.
3. Wait up to 500 ms for a response.
4. Interpret response Data byte 0:

| Value | Behavior |
|-------|----------|
| `0x01` | Mark the lens standalone and load descriptor/settings data |
| `0x02` | Mark a camera body attached and still load descriptor/settings data |
| `0x03` | Enter the recovery firmware flow |
| Other | Perform no data load or connection-state transition |

A timeout closes the connection attempt. The camera-attached path is enabled;
it is not an optional mode. Individual reads or writes may subsequently fail
with result `0x83`. Firmware update is refused before transfer while a body
is marked attached.

The explicit user disconnect path sends Op `0xF9`, waits up to 500 ms, and
then changes the user-visible state. It does not itself close the serial port
or reset the sequence counter. Transport teardown, including final firmware
cleanup and removal handling, closes the port and resets the sequence without
first requiring Op `0xF9`.

### 5.2 Memory reads

The whole-block request is:

```text
Op 0xF4
Data [0x00, region, block, 0x00, 0x00]
```

When the host performs a single-byte read, it emits:

```text
Op 0xF4
Data [0x00, region, 0x00, offset, 0x01]
```

All emitted single-byte reads use block zero. No word-read request is emitted.

After a successful result, the host skips response Data bytes 1 through 4 and
uses bytes from Data offset 5 onward.

### 5.3 Memory writes

Ordinary byte and word writes are:

```text
byte: [0x00, 0x01, block, offset, 0x01, value]
word: [0x00, 0x01, block, offset, 0x02, value_lo, value_hi]
```

When restoring the normal 512-byte settings image, the host emits these two
asymmetric requests exactly:

| Block | Selector | Appended payload |
|-------|----------|------------------|
| 0 | `[0x00, 0x01, 0x00, 0x00, 0x00]` | All 512 image bytes |
| 1 | `[0x00, 0x01, 0x01, 0x00, 0x00]` | Image bytes 256 through 511 |

Thus the first restore frame declares `size = 0` and carries 512 payload
bytes; it is not a 256-byte block write. The second carries 256 bytes. A
compatible implementation matching this behavior must not normalize the
first request to 256 bytes.

Factory initialization is one write-memory frame:

```text
Op 0xF5
Data [0x01, 0x01, 0x00, 0x00, 0x00]
```

After a successful initialization response, descriptor and settings data are
loaded again.

### 5.4 Initial data load

Both standalone and camera-attached connections perform:

1. Read descriptor region block 0 as a whole block.
2. Read settings region block 0 as a whole block.
3. Read settings region block 1 as a whole block.
4. Skip the five response Data prefix bytes for each read.
5. Concatenate settings block 0 followed by block 1.

---

## 6. Descriptor Block

The descriptor is the 256 bytes returned for region `0x00`, block 0. Offsets
below are relative to the first byte after the five-byte response prefix.

| Offset | Size | Host interpretation |
|--------|------|---------------------|
| 0 | 1 | Lens flags: bit `0x02` selects single-focal classification; bit `0x04` selects a back-ring zoom classification; otherwise front-ring zoom |
| 1 | 1 | Firmware flags: bit 0 changes removal/apply handling; bit 1 enables the staged firmware start described in Section 8.1 |
| 3 | 1 | Button-presence count used for device presentation; it does not bound the four settings slots in Section 7 |
| 4 | 1 | Number of custom-switch positions used for presentation and slot visibility |
| 5 | 1 | Mount selector: 0 Sony E, 1 Canon RF, 2 Nikon Z; any other value leaves the prior classification unchanged (initially Sony E) |
| 7 | 1 | Focus-ring angle-index range: high nibble minimum, low nibble maximum |
| 8 | 1 | Unsigned calibration half-range `P`; zero is treated as 1, and the signed setting is clamped to `[-P, +P]` |
| 9 | 1 | Maximum focus-motor speed index |
| 10 | 1 | Duration limit source; maximum tens-of-seconds digit is `floor(value / 10)`, with the editor also capped at 9 |
| 11 | 1 | Iris-angle index range: high nibble minimum, low nibble maximum |
| 15 | 1 | Bit 0 fixes the displayed AF far limit to infinity; storage still uses the last position-table index |
| 16..23 | 8 | Model identifier, byte characters terminated by zero |
| 24 | 1 | Firmware minor byte |
| 25 | 1 | Firmware major byte |
| 32..35 | 4 | Feature-capability bits; see Section 6.1 |
| 40 | 1 | Custom-switch mode capability bits; bit index equals the mode value |
| 48..51 | 4 | Button-function capability bits for IDs 0 through 31 |
| 64..79 | 16 | AF-limit position descriptors; see Section 6.2 |
| 96..159 | 64 | Product name, byte characters terminated by zero |

The canonical firmware value is formed as two hexadecimal bytes
`major.minor`. For presentation, when major is at least 1 the host displays
only the two-digit hexadecimal major byte; otherwise it displays both bytes.

### 6.1 Consumed capability bits

Bit indices are little-endian within bytes 32 through 35. The host exposes
behavior for these bits:

| Bit | Gated behavior |
|-----|----------------|
| 0 | Button assignment |
| 1 | Switch assignment |
| 2 | Focus-ring function |
| 3 | Focus-ring direction |
| 4 | Focus-ring response |
| 5 | Focus-ring rotation angle |
| 7 | Focus-throw calibration |
| 9 | Focus-ring iris-angle setting |
| 10 | Pre-actuation delay |
| 12 | Manual-focus override sensitivity |

Bits 6, 8, 11, and 13 through 31 do not gate an exposed behavior in this
host and are intentionally left without semantic names.

### 6.2 AF-limit position descriptors

Entries are assigned indices 0 through 15 in table order:

- Index 0 is always added. Value `0x00` is presented as the nearest limit.
- At indices 1 through 15, value `0x00` terminates the table and is not
  added.
- Value `0xFF` is presented as infinity.
- Values 1 through 9 are presented as `0.x` metres.
- Values 10 through 254 are presented as `floor(value / 10)` metres.

---

## 7. Settings Image

Settings memory consists of two 256-byte blocks. Absolute offset is
`block * 256 + offset`.

The four logical assignment slots are:

| Slot | Role |
|------|------|
| 0 | Focus-set button |
| 1 | Custom-switch position 1 |
| 2 | Custom-switch position 2 |
| 3 | Custom-switch position 3 |

Slots 1 through 3 are not three additional independent physical buttons.
Their visibility follows the switch-position count in descriptor byte 4.

### 7.1 Block 0

| Absolute offset | Size | Interpretation |
|-----------------|------|----------------|
| 0 | 1 | Focus-ring function value |
| 1 | 1 | Focus-ring direction value |
| 2 | 1 | Focus-ring response value |
| 3 | 1 | Focus-ring angle index; displayed angle is `(value + 1) * 90` degrees |
| 5 | 1 | Focus-ring iris-angle index; displayed angle is `value * 15 + 45` degrees |
| 9 | 1 | Signed manual-focus override sensitivity |
| 16 | 1 | Switch AF-limit position 1 and slot 0 AF-limit |
| 17 | 1 | Switch AF-limit position 2 |
| 18 | 1 | Switch AF-limit position 3 |
| 19 | 1 | Signed focus-throw calibration value |
| 64 | 1 | Custom-switch mode; see Section 7.2 |
| 80..83 | 1 each | Function value for logical slots 0 through 3 |
| 84, 86, 88, 90 | 2 each | Skip-exposure count for slots 0 through 3 |
| 92, 94, 96, 98 | 2 each | Move count for slots 0 through 3 |
| 96..99 | 1 each | Focus-motor speed for slots 0 through 3 |
| 100, 102, 104, 106 | 2 each | Actuation tally for slots 0 through 3 |
| 224, 225, 226 | 1 each | AF-limit byte for slots 1 through 3 |

Skip-exposure and move counts are composed from four decimal digits and
stored as ordinary little-endian integers from 0 through 9999. A loaded value
above 9999 is presented as 9999. A loaded move count of zero is presented as
1.

Offsets 96 through 99 have overlapping interpretations:

- Move-count slot 2 is the word at 96..97.
- Move-count slot 3 is the word at 98..99.
- All four bytes are also read individually as slot speed indices.

Both interpretations are applied unconditionally. No exclusivity rule is
enforced. A writer must therefore account for the overlap rather than assume
that only one interpretation can apply.

Changing a slot's skip-exposure or move count is followed by a separate word
write of zero to that slot's actuation tally at `100 + 2 * slot`.

### 7.2 Enumerated settings bytes

The focus-ring fields at block-0 offsets 0 through 2 use these values:

| Offset | Value | Meaning |
|-------:|------:|---------|
| 0 | `0x00` | Manual focus |
| 0 | `0x01` | Manual iris |
| 1 | `0x00` | Forward direction |
| 1 | `0x01` | Reverse direction |
| 1 | `0x02` | Follow camera direction |
| 2 | `0x00` | Nonlinear response |
| 2 | `0x01` | Linear response |

The custom-switch mode at block-0 offset 64 uses these values:

| Value | Mode |
|------:|------|
| `0x00` | AF limit |
| `0x01` | AF limit for positions 1 and 2, manual focus for position 3 |
| `0x04` | Multi-select; positions 1 through 3 select logical slots 1 through 3 |

The logical-slot function bytes at block-0 offsets 80 through 83 use these
values. Value `0x15` (21) has no assigned function.

| Value | Function | Value | Function |
|------:|----------|------:|----------|
| `0x00` | None | `0x0F` | Infinity lock, click |
| `0x01` | AF/MF switch, long press | `0x10` | MF limit, long press |
| `0x02` | AF/MF switch, click | `0x11` | MF limit, click |
| `0x03` | AF limit, long press | `0x12` | VC on/off, long press |
| `0x04` | AF limit, click | `0x13` | VC on/off, click |
| `0x05` | Focus hold | `0x14` | Timelapse, long press |
| `0x06` | AF limit while pressed | `0x16` | Timed focus preset |
| `0x07` | AF limit while released | `0x17` | Timed A-B focus |
| `0x08` | Focus preset | `0x18` | Timed iris preset |
| `0x09` | A-B focus | `0x19` | Timed A-B iris |
| `0x0A` | Focus hold 2 | `0x1A` | MF response switch, long press |
| `0x0B` | Adjustable infinity lock | `0x1B` | MF response switch, click |
| `0x0C` | Ring switch, long press | `0x1C` | Ring stopper, long press |
| `0x0D` | Ring switch, click | `0x1D` | Ring stopper, click |
| `0x0E` | Infinity lock, long press |  |  |

Support for a stored switch mode or button function is lens-specific. Switch
mode support is advertised by descriptor byte 40, and button-function support
by descriptor bytes 48 through 51, with each function value serving as its bit
index. Button function values `0x18` and `0x19` use the iris timing fields in
Section 7.3 when their capability bits are set.

### 7.3 Block 1 timing fields

For logical slot `s` from 0 through 3:

| Block offset | Size | Interpretation |
|--------------|------|----------------|
| `32 + 2*s` | 2 LE | Focus-motor actuation duration |
| `40 + 2*s` | 2 LE | Iris-motor actuation duration |
| `48 + 2*s` | 2 LE | Focus-motor pre-actuation delay |
| `56 + 2*s` | 2 LE | Iris-motor pre-actuation delay |

For a duration word `W = 100*A + 10*B + C`, the displayed duration is:

```text
(10*A + B).C seconds = W / 10 seconds
```

`A` is limited by descriptor byte 10 as described in Section 6.

For a delay word `W = 10*A + B`, the displayed delay is:

```text
A.B seconds = W / 10 seconds
```

Timed focus functions use the focus-motor fields. Timed iris functions use the
iris-motor fields.

### 7.4 AF-limit packing

Every AF-limit byte uses:

```text
value = ((far_index << 4) & 0xF0) | (near_index & 0x0F)
```

Both nibbles are indices into the descriptor table in Section 6.2. On read,
an index beyond the populated table is clamped to the last populated index.

When descriptor byte 15 bit 0 is set, the host restricts the far selection to
the last populated descriptor entry and presents it as infinity. The stored
far nibble is that last entry's index. Far index zero does not mean fixed
infinity; it selects descriptor index zero.

---

## 8. Firmware Transfer

Firmware transfer begins with one command frame and then changes to raw
control bytes and 1029-byte data frames.

### 8.1 Start sequence

1. Refuse to start if the connection is marked camera-attached.
2. Send command Op `0xFD` with Data `[device, area]`.
3. Do not wait for a command response to Op `0xFD`.
4. Enter firmware-transfer receive mode, wait 10 ms, change to 3000000 baud,
   and wait up to 10000 ms for byte `0x43`.
5. If `0x43` is not received, restore 19200 baud and fail the transfer.

`device` and `area` are byte selectors supplied by the firmware image.
The recovery path selected by connect result `0x03` uses
`[device = 0, area = 0]`.

When descriptor byte 1 bit 1 is set, a non-recovery transfer for device zero
forces area to 2 and performs a staged start:

1. Send Op `0xFD` with `[0, 2]`.
2. Attempt to re-establish the serial connection up to five times, waiting
   600 ms before each attempt.
3. After reconnection, send Op `0xF8` and wait up to 500 ms.
4. Wait 100 ms, then send Op `0xFD` with `[0, 2]` again.
5. Continue with the baud-rate change and `0x43` wait above.

No response to either staged Op `0xFD` request is awaited.

### 8.2 Data frame

Each firmware data frame is exactly 1029 bytes:

```text
+-------+---------+------------------+---------------------+--------+--------+
| 0x02  | Block # | ~Block #        | Data                | CRC Hi | CRC Lo |
| 1 B   | 1 B     | 1 B             | 1024 bytes          | 1 B    | 1 B    |
+-------+---------+------------------+---------------------+--------+--------+
```

| Field | Behavior |
|-------|----------|
| Start | `0x02` |
| Block number | Counter modulo 255 after increment |
| Complement | Ones-complement of the emitted block number |
| Data | Exactly 1024 bytes |
| CRC | CRC-16/XMODEM over only the 1024 Data bytes, high byte first |

Block numbers are emitted as:

```text
01, 02, ..., FE, 00, 01, ...
```

There is no trailing delimiter.

### 8.3 Control bytes

| Byte | Direction | Actual host behavior |
|------|-----------|----------------------|
| `0x43` | Lens to host | Releases the initial send-request wait |
| `0x06` | Lens to host | Releases the shared response wait used for block and final ACKs, but only after at least one block was emitted |
| `0x15` | Lens to host | Releases the NAK wait used after EOT; during a block ACK wait it does not request retransmission, so that block attempt times out |
| `0x18` | Lens to host | Recognized as cancel but releases neither the block ACK wait nor the completion NAK wait; the active wait times out |
| `0x04` | Host to lens | End-of-transfer marker |

The host does not automatically retransmit a block after `0x15`, does not
immediately abort a block wait after `0x18`, and does not emit a cancel byte
in the reachable transfer flow.

### 8.4 Data sequence

The on-wire data does not begin with firmware file byte zero:

1. Allocate a zero-filled 1024-byte buffer and send it without reading the
   firmware stream. This is block number 1.
2. Wait up to 10000 ms for `0x06`.
3. For each following block, allocate a new zero-filled 1024-byte buffer and
   read up to 1024 firmware bytes into it.
4. If the read returns zero bytes, stop without sending that buffer.
5. If the read is partial, leave the unused tail as `0x00`.
6. Send the full 1024-byte buffer and wait up to 500 ms for `0x06`.
7. If the ACK wait times out, fail the transfer without retrying the block.

Consequently, every firmware area is preceded by a 1024-byte all-zero block,
and its final partial file block is padded with `0x00`.

### 8.5 Completion sequence

After all file data blocks for one firmware area:

1. Send `0x04`.
2. Wait up to 500 ms for `0x15`.
3. Send `0x04` again.
4. Wait up to 500 ms for `0x06`.
5. Leave firmware-transfer receive mode and restore 19200 baud.

Failure of either wait fails the area. After all requested areas complete or
one fails, final cleanup closes the serial connection.

---

## 9. Timing Summary

| Event | Timeout or delay |
|-------|------------------|
| Ordinary command response | 500 ms |
| Staged reconnect | Up to 5 attempts, 600 ms before each |
| Delay between staged connect response and second begin request | 100 ms |
| Delay before changing to transfer baud | 10 ms |
| Initial `0x43` wait | 10000 ms |
| Initial zero-block ACK | 10000 ms |
| Each file-block ACK | 500 ms |
| First EOT NAK | 500 ms |
| Second EOT ACK | 500 ms |

---

## 10. Setting Write Reference

All ordinary setting changes use Op `0xF5`, region `0x01`.

| Setting | Block | Offset | Size | Encoding or side effect |
|---------|-------|--------|------|-------------------------|
| Focus-ring function | 0 | 0 | 1 | Section 7.2 enumeration |
| Focus-ring direction | 0 | 1 | 1 | Section 7.2 enumeration |
| Focus-ring response | 0 | 2 | 1 | Section 7.2 enumeration |
| Focus-ring angle index | 0 | 3 | 1 | Display `(value + 1) * 90` degrees |
| Focus-ring iris-angle index | 0 | 5 | 1 | Display `value * 15 + 45` degrees |
| Manual-focus override sensitivity | 0 | 9 | 1 | Signed byte |
| Switch AF-limit position `i` | 0 | `16 + i` | 1 | Packed far/near indices, `i = 0..2` |
| Focus-throw calibration | 0 | 19 | 1 | Signed byte |
| Custom-switch mode | 0 | 64 | 1 | Section 7.2 enumeration |
| Logical slot `s` function | 0 | `80 + s` | 1 | Section 7.2 enumeration; `s = 0..3` |
| Logical slot `s` skip count | 0 | `84 + 2*s` | 2 | Little-endian; followed by tally reset |
| Logical slot `s` move count | 0 | `92 + 2*s` | 2 | Little-endian; followed by tally reset |
| Logical slot `s` speed | 0 | `96 + s` | 1 | Overlaps move-count words for slots 2 and 3 |
| Logical slot `s` AF-limit | 0 | 16 for `s=0`; `223+s` for `s=1..3` | 1 | Packed far/near indices |
| Timed-focus slot `s` duration | 1 | `32 + 2*s` | 2 | Little-endian tenths of a second |
| Timed-iris slot `s` duration | 1 | `40 + 2*s` | 2 | Little-endian tenths of a second |
| Timed-focus slot `s` pre-delay | 1 | `48 + 2*s` | 2 | Little-endian tenths of a second |
| Timed-iris slot `s` pre-delay | 1 | `56 + 2*s` | 2 | Little-endian tenths of a second |
| Factory initialization | - | - | - | Data `[0x01, 0x01, 0x00, 0x00, 0x00]` |

Whole-image restore is not an ordinary setting write and must use the exact
asymmetric payloads in Section 5.3.
