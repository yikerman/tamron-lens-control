# Tamron Lens Firmware Update - Clean Protocol Specification

This document specifies the externally reproducible firmware-update behavior
for a compatible Tamron lens. It covers server discovery, firmware-container
encoding and decoding, image selection, and the serial transfer to the lens.

Names used here are descriptive only. Numeric values, byte layouts, ordering,
timeouts, and observable behavior are normative. Decoded firmware contents are
treated as opaque image bytes; knowledge of their internal program structure is
not required to implement this protocol.

Unless otherwise stated, multi-byte integers are little-endian. CRC-16 means
the non-reflected CRC-16/XMODEM variant with polynomial `0x1021`, initial value
`0x0000`, and no final XOR.

Firmware update is destructive and interruption can make a lens unusable. An
implementation should keep update support separate from ordinary read-only and
settings operations, require explicit user confirmation, and never exercise
the update path in an automated hardware test.

---

## 1. End-to-End Flow

A normal network update consists of these phases:

1. Connect to the lens in command mode at 19200 baud.
2. Read the 256-byte descriptor block and obtain the model identifier, mount
   selector, current firmware bytes, and firmware flags.
3. Fetch the model-and-mount XML manifest from the update server.
4. Compare the advertised version with the version presented for the lens.
5. After user confirmation, fetch the firmware container named by the
   manifest.
6. Decode the container, validate every embedded image CRC, and select/order
   the images for the requested update mode.
7. For each selected image, enter firmware-transfer mode and transmit it using
   the raw 1024-byte block protocol.
8. On success or failure, leave firmware mode, restore 19200 baud, and close
   the serial transport.

The firmware payload is downloaded and decoded before any firmware data is
sent to the lens. Decoding is transactional: retain decoded records in a
private staging collection and publish them for transfer only after the
complete input reaches a normal terminator and every record passes validation.
A container decode or CRC failure must therefore stop the operation before the
lens-transfer phase begins, with no staged record transferred.

---

## 2. Lens Identity Used for Server Discovery

The descriptor block is read as specified in `PROTOCOL.md`. Relevant
fields are:

| Descriptor offset | Size | Use |
|-------------------|------|-----|
| 1 | 1 | Firmware flags |
| 5 | 1 | Mount selector |
| 16..23 | 8 | Model identifier, byte characters terminated by zero |
| 24 | 1 | Installed firmware minor byte |
| 25 | 1 | Installed firmware major byte |

The canonical installed firmware string is:

```text
hex2(major) + "." + hex2(minor)
```

where `hex2` emits two uppercase hexadecimal digits. For example, major
`0x01` and minor `0x00` produce `01.00`.

The server manifest suffix is derived from the mount classification:

| Mount | Manifest suffix |
|-------|-----------------|
| Sony E | `SE` |
| Canon RF | `RF` |
| Nikon Z | `Z` |
| Fujifilm X | `X` |

In the ordinary connected flow, descriptor selectors `0`, `1`, and `2`
select Sony E, Canon RF, and Nikon Z respectively. Other selector values leave
the previous mount classification unchanged; its initial value is Sony E. In
the recovery flow, the raw selector is used directly, so selector `3` selects
Fujifilm X.

The model string is used as received. No case folding or URL escaping is
applied when constructing the manifest path.

---

## 3. Update Server Protocol

### 3.1 Base URL

The compatibility endpoint is:

```text
http://tamron.cdngc.net/lensutility/lens/
```

The observed server also accepts the equivalent HTTPS endpoint:

```text
https://tamron.cdngc.net/lensutility/lens/
```

The compatibility behavior uses plain HTTP and the requests have no
application-specific authentication or required custom headers. A new
implementation should prefer HTTPS for both the manifest and firmware payload,
perform normal certificate and hostname validation, and reject redirects that
downgrade either request to HTTP. Both resources must use the same protected
transport because the manifest supplies the payload filename and decoding key.

HTTPS protects the delivery connection from ordinary interception and
modification. It does not establish end-to-end firmware authenticity: the
container uses XOR obfuscation and CRC-16 rather than a cryptographic
signature. A separately trusted signature or hash is still preferable when
one is available.

### 3.2 Manifest request

The manifest URL is:

```text
BASE + model + mount_suffix + ".xml"
```

Example for model `A068`, Sony E:

```text
http://tamron.cdngc.net/lensutility/lens/A068SE.xml
```

Request timing:

| Limit | Value |
|-------|-------|
| Response timeout | 5000 ms |
| Read/write timeout | 60000 ms |

The response is parsed as XML. Three element names are consumed:

| Element | Meaning |
|---------|---------|
| `Version` | Advertised firmware version |
| `FirmwareFileName` | Relative payload filename appended to `BASE` |
| `DescriptionKey` | 64-character uppercase hexadecimal container-key field |

Other elements are ignored. Element text is used directly without trimming,
normalization, or path validation.

An HTTP 404 is treated as a completed metadata lookup, but leaves the cleared
defaults in place:

```text
Version          = "--"
FirmwareFileName = ""
DescriptionKey   = ""
```

An update attempt must reject incomplete metadata before constructing or
requesting a firmware payload. In particular, an empty `FirmwareFileName` or
an empty or invalid `DescriptionKey` is a metadata/data failure, including in
the recovery flow. A completed 404 lookup is therefore not permission to
request `BASE` or to enter firmware-transfer mode.

Other HTTP failures, timeouts, malformed XML, and parsing failures make the
metadata lookup fail.

### 3.3 Version presentation and update indication

The advertised version has a separate display form:

- If it contains a dot and the first component is neither `0` nor `00`, only
  the first component is displayed.
- Otherwise the full advertised value is displayed.

The installed display value is the two-digit major byte alone when the major
byte is at least 1; otherwise it is the full `major.minor` string.

An update is indicated when metadata lookup completed, the advertised display
value is not the placeholder `--`, and these two display strings differ. An
advertised display value of `--` (for example after an HTTP 404) never
indicates an update, even though it differs from any installed display value.
The indication is a string comparison, not a semantic version-ordering
comparison. It can therefore indicate an update for any other difference,
including an older advertised value.

### 3.4 Firmware request

The payload URL is constructed exactly as:

```text
BASE + FirmwareFileName
```

Request timing:

| Limit | Value |
|-------|-------|
| Response timeout | 5000 ms |
| Read/write timeout | 180000 ms |

The response body is decoded as a stream. It is not required to be saved to a
temporary file. Network and timeout failures are classified as download
failures. Key-format, container-layout, payload-CRC, and local processing
failures are classified as firmware-data failures.

---

## 4. Firmware Container

### 4.1 Overall layout

A container is a concatenation of one or more records with no global header,
record count, index, footer, or trailing checksum:

```text
record_0 || record_1 || ... || record_n
```

Each record is:

```text
encrypted_header[1024] || encrypted_payload[payload_size]
```

`payload_size` is obtained only after decoding the 1024-byte header. End of
file immediately after a complete payload is the normal container terminator.

The decoder rejects:

- A final partial header.
- A payload size greater than 1,048,576 bytes.
- A payload shorter than the decoded size.
- A decoded payload whose CRC does not equal the header CRC.
- A missing, malformed, or non-uppercase key field.

An empty container contains no usable image and cannot start an update.

### 4.2 Record header

The decoded header is exactly 1024 bytes. These offsets are consumed:

| Offset | Size | Encoding | Meaning |
|--------|------|----------|---------|
| 64 | 4 | Little-endian unsigned | Payload size in bytes |
| 70 | 2 | CRC byte order described below | Payload CRC-16 |
| 72 | 1 | Unsigned byte | Device selector |
| 73 | 1 | Unsigned byte | Area selector |

All other header bytes are opaque to the update protocol. This includes
offsets 68 and 69, which carry an image version in observed containers but
are never read by the update flow. A decoder must not require a model name,
version, magic string, zero fill, or any other value in those unused
positions.

The payload CRC bytes use the return/storage order of the common CRC routine:

```text
offset 70 = CRC low byte
offset 71 = CRC high byte
```

The CRC input is exactly the decoded payload bytes, excluding the header.

### 4.3 Description-key validation

The manifest `DescriptionKey` must satisfy both conditions:

```text
length == 64
all characters match [0-9A-F]
```

Lowercase hexadecimal is rejected. Each adjacent character pair is converted
to one byte, producing 32 bytes:

```text
description_bytes[i] = hex(DescriptionKey[2*i : 2*i+2])
```

### 4.4 Derived XOR key

Construct a 32-byte base sequence by repeating the ASCII byte string below
and truncating it to 32 bytes:

```text
EncryptionToolBaseKeyEncryptionT
```

The record-stream key is:

```text
key[i] = description_bytes[i] XOR base_sequence[i]
```

This operation is symmetric. The same derived key is used for container
encoding and decoding.

### 4.5 Stream transformation

Initialize `key_index = 0` once for the entire container. For every encrypted
or plaintext byte, in file order:

```text
output_byte = input_byte XOR key[key_index]
key_index = (key_index + 1) modulo 32
```

The key index continues across:

- Header to payload.
- Payload to the next record header.
- Every subsequent record in the same container.

It is not reset at a record boundary. Because every header is 1024 bytes,
which is divisible by 32, header processing alone returns the key index to the
same position. A payload whose size is not divisible by 32 shifts the starting
key position of the next record.

### 4.6 Decoder algorithm

```text
validate DescriptionKey
derive key[32]
key_index = 0
records = []

while encrypted input remains:
    read exactly 1024 bytes
    if fewer than 1024 bytes remain:
        fail as malformed container

    header = xor_stream(header_bytes)
    size   = little_endian_u32(header[64:68])
    if size > 1048576:
        fail as malformed container

    read exactly size bytes
    if fewer than size bytes remain:
        fail as malformed container

    payload = xor_stream(payload_bytes)
    expected_crc = little_endian_u16(header[70:72])
    if crc16_xmodem(payload) != expected_crc:
        fail as corrupt container

    append record(device=header[72], area=header[73], payload=payload)

if records is empty:
    fail
```

The decode operation is all-or-nothing for update purposes. Decode into a
temporary collection and commit it only when the loop terminates normally with
at least one complete record. If any later header, payload, size, or CRC check
fails, discard every staged record and do not contact the lens. This rule also
applies when a stream or local processing exception occurs during decoding.

### 4.7 Encoder algorithm

Encoding uses the same byte transformation and continuous key position:

```text
validate DescriptionKey
derive key[32]
key_index = 0

for each record in order:
    require payload length <= 1048576
    construct a 1024-byte header
    header[64:68] = little_endian_u32(payload length)
    header[70:72] = little_endian_u16(crc16_xmodem(payload))
    header[72]    = device
    header[73]    = area

    emit xor_stream(header)
    emit xor_stream(payload)
```

Bytes outside the consumed header fields are not interpreted by the update
protocol. For a new clean encoder, initialize them to zero unless a separately
documented ecosystem requirement supplies values. Decoding and re-encoding an
existing container should preserve the original opaque header bytes if
byte-for-byte reproduction is required.

This container transformation provides obfuscation and accidental-corruption
detection, not cryptographic authenticity. A clean implementation should not
describe a successful XOR/CRC check as signature verification.

---

## 5. Image Selection and Ordering

Every decoded record supplies its own device and area selectors. These bytes
are passed to the lens-transfer start command, subject to the staged-start
override in Section 6.2.

### 5.1 Normal network update

All decoded records are transferred. If there are at least two records and a
record with `(device = 0, area = 0)` exists, that record is moved to the end
while the relative order of the other records is retained.

This ordering means auxiliary device/area images are attempted before the
primary `(0,0)` image.

### 5.2 Recovery update

Select only the first record whose selectors are:

```text
device = 0
area   = 0
```

Fail before lens transfer if no such record exists. Discard every other
decoded record for this update attempt.

### 5.3 Local raw-image maintenance

A local maintenance path accepts one raw binary file instead of fetching and
decoding a server container. The selected file is opened as an opaque byte
stream and mapped to one selector pair:

| Maintenance target | Device | Area |
|--------------------|--------|------|
| Primary application image | 0 | 0 |
| Primary boot image | 0 | 1 |
| Stabilization subsystem image | 1 | 0 |

The file selector filters for a `.bin` extension, but the transfer path does
not validate a filename, model identifier, embedded version, expected length,
header, CRC, signature, or target compatibility before sending it. File length
is used only for progress calculation.

After selection, the raw file follows the same lens start, initial zero block,
1024-byte zero-padded data frames, ACK waits, and EOT completion sequence as a
decoded container record. The descriptor firmware flag can still force a
non-recovery device-zero maintenance target to staged area `2`, replacing the
table's original area.

Canceling the local file picker stops the maintenance operation before
firmware mode begins. These targets are maintenance behavior rather than the
ordinary server update and should be hidden behind an explicit expert or
recovery interface. A clean implementation should add its own strict model,
target, size, and hash allowlisting before making these paths available.

### 5.4 Progress denominator

For all selected records, the total progress size is:

```text
sum(payload_size + 1024)
```

The added 1024 bytes accounts for the mandatory initial zero block sent for
each image. Progress advances by exactly 1024 after every acknowledged block,
including final padded blocks, so the displayed percentage can be based on
transmitted block bytes rather than exact unpadded payload bytes.

---

## 6. Lens Transfer Start

The normal command frame format, sequence handling, and CRC are defined in
`PROTOCOL.md`.

### 6.1 Ordinary start

Preconditions:

- The serial transport is open in command mode at 19200 baud, 8N1.
- A camera body is not marked attached.
- The complete container has decoded and all selected payload CRCs have
  passed.

Start one selected image as follows:

1. Send command Op `0xFD` with Data `[device, area]`.
2. Do not wait for a command-mode response to Op `0xFD`.
3. Change receive interpretation from command frames to raw firmware control
   bytes.
4. Wait 10 ms.
5. Change the existing serial port to 3000000 baud.
6. Wait up to 10000 ms for raw byte `0x43`.
7. If `0x43` is not received, restore 19200 baud and fail the image.

### 6.2 Staged start for an OS-inclusive image

Descriptor byte 1 bit 1 enables a staged start. It applies when:

```text
update is not the recovery flow
AND requested device == 0
```

When it applies, replace the requested area with `2` and perform:

1. Send Op `0xFD` with Data `[0, 2]` at 19200 baud.
2. Do not wait for a response to this request.
3. Attempt to reopen/re-establish the serial connection up to five times,
   sleeping 600 ms before each attempt.
4. After one successful reopen, send Op `0xF8` with Data `[0x00]` and wait up
   to 500 ms for its normal response.
5. Wait 100 ms.
6. Send Op `0xFD` with Data `[0, 2]` again, without waiting for its response.
7. Enter raw firmware receive mode, wait 10 ms, switch to 3000000 baud, and
   wait up to 10000 ms for `0x43`.

Failure to reopen within five attempts, failure of the connect response, or
failure to receive `0x43` fails the image. The second start uses area `2`; it
does not restore the record's original area.

### 6.3 Recovery start

When command-mode connection result `0x03` indicates recovery state, metadata
is fetched using the model and raw mount selector read from the descriptor.
The recovery flow must verify that the lookup produced a non-empty firmware
filename and a valid description key before requesting the payload. Only the
`(0,0)` container record is selected. The staged area-2 override is disabled
even if descriptor byte 1 bit 1 is set.

---

## 7. Raw Firmware Transfer

### 7.1 Control bytes

| Byte | Direction | Meaning and host behavior |
|------|-----------|---------------------------|
| `0x43` | Lens to host | Initial request to begin CRC-mode transfer |
| `0x06` | Lens to host | ACK; releases a block or final-ACK wait after at least one block has been emitted |
| `0x15` | Lens to host | NAK; releases the completion NAK wait, but not a block ACK wait |
| `0x18` | Lens to host | Cancel indication; recognized but releases no active transfer wait |
| `0x04` | Host to lens | End-of-transfer marker |

The reachable update flow never sends `0x18` to cancel a transfer.

### 7.2 Data-frame layout

Every block frame is exactly 1029 bytes:

```text
+-------+---------+------------------+---------------------+--------+--------+
| 0x02  | Block # | ~Block #        | Data                | CRC Hi | CRC Lo |
| 1 B   | 1 B     | 1 B             | 1024 bytes          | 1 B    | 1 B    |
+-------+---------+------------------+---------------------+--------+--------+
```

| Field | Rule |
|-------|------|
| Start | Always `0x02` |
| Block number | Increment internal counter, then reduce modulo 255 |
| Complement | Ones-complement of the emitted block number |
| Data | Exactly 1024 bytes |
| CRC | CRC-16/XMODEM over only Data, high byte followed by low byte |

There is no trailing delimiter. The firmware-frame CRC byte order is opposite
the command-frame and container-header CRC storage order.

The emitted block-number sequence is:

```text
01, 02, ..., FE, 00, 01, ...
```

The internal firmware block counter begins at zero with a new transport
context. There is no explicit counter reset between selected records or during
normal firmware cleanup, so records transferred through the same context
continue the sequence. If staged re-enumeration causes that context to be
discarded and recreated, the new context begins again at zero.

### 7.3 Payload sequence

For each selected image:

1. Allocate a new zero-filled 1024-byte buffer without reading the image.
2. Send that all-zero buffer as the image's first data frame.
3. Wait up to 10000 ms for `0x06`.
4. Repeatedly allocate a new zero-filled 1024-byte buffer and read up to 1024
   image bytes into its beginning.
5. If the read returns zero bytes, stop sending data frames.
6. Otherwise send the entire 1024-byte buffer and wait up to 500 ms for
   `0x06`.
7. Stop and fail on a timeout or non-ACK completion of the wait.

Consequences:

- The first on-wire block is not image data; it is 1024 zero bytes.
- Actual image byte zero appears in the following frame.
- A final partial image block is padded with `0x00`.
- An image whose size is an exact multiple of 1024 gets no extra trailing
  data frame.
- A zero-length image sends only the initial zero block and then proceeds to
  completion.

No block is automatically retransmitted. A raw `0x15` received while waiting
for a block ACK does not satisfy that ACK wait, so the attempt eventually
fails by timeout. A raw `0x18` also does not release the wait and eventually
causes a timeout.

The result of the underlying serial write is not independently acknowledged
by the host-side state machine. After attempting a block or EOT write, the host
still waits for the corresponding lens control byte. A local write failure is
therefore normally observed as the applicable receive timeout.

### 7.4 Completion sequence

After all image bytes have been acknowledged:

1. Send raw `0x04`.
2. Wait up to 500 ms for raw `0x15`.
3. Send raw `0x04` again.
4. Wait up to 500 ms for raw `0x06`.

Failure of either wait fails the image. On success, that image is complete and
the next selected record, if any, begins again with the command-mode start
sequence described in Section 6.

### 7.5 Cleanup

After each image attempt, leave raw firmware receive interpretation and restore
the serial port to 19200 baud. Put this cleanup in an unconditional guard that
runs for success, protocol failure, transport failure, cancellation, and
unexpected exceptions. After all selected images complete or one fails, close
the lens transport. Closing the transport resets the ordinary command sequence
counter. If cleanup itself fails, report the update as failed while still
attempting the remaining cleanup actions.

---

## 8. Failure Semantics

| Phase | Failure | Required outcome |
|-------|---------|------------------|
| Manifest | Non-404 HTTP error, timeout, or invalid XML | Report metadata/network failure; do not update |
| Manifest | HTTP 404 | Complete lookup with empty/default fields; reject the update before payload request |
| Payload fetch | HTTP error or timeout | Report download/network failure; do not contact firmware mode |
| Container | Invalid key format, short header/payload, excessive size, CRC mismatch, stream/local processing error, or no records | Discard all staged records; report firmware-data failure; do not contact firmware mode |
| Selection | Recovery container lacks `(0,0)` | Report firmware-data failure; do not contact firmware mode |
| Lens start | Reconnect, connect-response, or `0x43` timeout | Fail current image and update |
| Data | Missing block ACK | Fail current image; do not retry the block |
| Completion | Missing NAK or final ACK | Fail current image |
| Multi-image update | Any image fails | Stop; do not attempt later images |

All terminal success and failure paths first leave firmware mode, restore
19200 baud, and then close the serial connection, including paths caused by
exceptions. A local file-selection cancellation is distinct: it ends the local
operation without starting firmware mode and need not disconnect an already
open lens session; if firmware mode was already entered, the unconditional
cleanup still applies.

---

## 9. Timing Summary

| Event | Timeout or delay |
|-------|------------------|
| Manifest response | 5000 ms |
| Manifest read/write | 60000 ms |
| Firmware response | 5000 ms |
| Firmware read/write | 180000 ms |
| Ordinary command response | 500 ms |
| Delay before each staged reconnect attempt | 600 ms |
| Maximum staged reconnect attempts | 5 |
| Delay before second staged start | 100 ms |
| Delay before switching to transfer baud | 10 ms |
| Initial raw `0x43` | 10000 ms |
| Initial zero-block ACK | 10000 ms |
| Image-data block ACK | 500 ms |
| First EOT NAK | 500 ms |
| Second EOT ACK | 500 ms |

---

## 10. Observed A068SE 03.00 Test Vector

The following public-server sample was retrieved on 2026-08-12. It is useful
for deterministic decoder tests but does not impose model-specific values on
other containers.

Manifest path:

```text
/lensutility/lens/A068SE.xml
```

Manifest values:

```text
Version          = 03.00
FirmwareFileName = A068SE_0300.tfwf
DescriptionKey   = 045E554A2A3502595C5E642E5F5A7A3236137B5649752F53444123311F5F5D64
```

Container facts:

| Property | Value |
|----------|-------|
| Encoded size | 523264 bytes (`0x7FC00`) |
| Encoded SHA-256 | `d9540d7b6d40c2abc97697fa64d477a1b0394cf7bd95538af1481ab66cc9f9f8` |
| Derived XOR key, hex | `4130363853457630333030413036385345763033303041303638534576303330` |
| Record count | 1 |

Decoded record:

| Property | Value |
|----------|-------|
| Device | 0 |
| Area | 0 |
| Payload size | 522240 bytes (`0x7F800`) |
| Version bytes | `03 00` |
| Header CRC bytes | `77 92` |
| CRC numeric value | `0x9277` |
| CRC validation | Pass |
| Payload SHA-256 | `e5f54f89414536818bce72cbe131b15cd35e3b8595e3599238aa409accaba36a` |

The decoded header begins with model-identifying bytes in this sample, but
the update protocol consumes only the fields listed in Section 4.2. A
conforming decoder must treat the rest of the header, including the version
bytes, and the complete decoded payload as opaque.

---

## 11. Implementation Checklist

- Keep server retrieval, container processing, and lens transfer as separate
  modules with typed inputs and outputs.
- Fetch and validate the complete container before sending Op `0xFD`.
- Reject lowercase, short, or non-hexadecimal description keys.
- Preserve the XOR key index continuously across all records.
- Enforce the 1 MiB per-record payload limit before allocation or read.
- Validate every payload CRC before selecting any image for transfer.
- Preserve record order, moving `(0,0)` last only in a multi-record normal
  update.
- Use raw `(0,0)` only in recovery mode.
- Treat local raw-image targets as unvalidated maintenance writes and require
  independent model/target/hash controls before exposing them.
- Implement the area-2 staged start only for non-recovery device-zero images
  when descriptor firmware flag bit 1 is set.
- Send one all-zero 1024-byte block before each image.
- Use block numbers modulo 255 and retain the counter while the same transport
  context remains active.
- Use high-byte-first CRC in raw data frames and low-byte-first CRC in
  container headers and command frames.
- Do not add retransmission behavior if exact compatibility is required.
- Always restore 19200 baud and close the transport after terminal completion.
- Never expose firmware update through an unrestricted arbitrary-write API.
