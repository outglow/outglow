# Outglow

Open firmware for Zigbee smart bulbs built on the Telink TLSR8258 chip, such
as the **EGLO Connect.Z** filament bulbs (AwoX modules). It replaces the
closed vendor firmware **over the air**, without opening the bulb: the first
time over Bluetooth LE from the original firmware, after that over Zigbee OTA.

Written from scratch in Rust, with no vendor code: the chip start-up, the
802.15.4 and BLE radio drivers, a Zigbee PRO stack and the light itself.

> **Status: experimental.** Outglow runs on a real bulb in a real home network,
> but on one bulb so far. Expect bugs. Flashing a bulb is at your own risk;
> read [Flashing](#flashing) first.

## What it does

- **Zigbee 3.0 router**: Basic, Identify, Groups, Scenes, On/Off, Level,
  Color Control (tunable white and colour lamps), OTA client, touchlink target,
  Green Power proxy, end-device parent. Network and APS security, install codes.
- **A router you can look into**: the ZCL Diagnostics cluster with all its
  counters plus manufacturer attributes (neighbour table with link quality,
  routes, event log, noise floor, stack depth, a black box of the last frames
  and attempts). A [Zigbee2MQTT converter](z2m/open-eglo.mjs) shows them.
- **Steadier routing**: link cost from measured delivery with smoothing and
  hysteresis, so routes do not flap.
- **Bluetooth LE kept**: the bulb still speaks the vendor app's profile, so the
  vendor app and the BLE update path keep working; BLE can be switched off or
  limited to a window after power-on from Zigbee.
- **Behaves like the original** where it matters: the same brightness and
  colour curves, the same power-on behaviour and factory-reset sequence
  (checked against the original firmware in the emulator).
- **21 boards** (white, tunable white and colour lamps):
  [`crates/bulb-tc32/boards.tsv`](crates/bulb-tc32/boards.tsv).

## Repository

| Path | What |
|---|---|
| `crates/zigbee` | Zigbee PRO stack, `no_std`, no allocation, no `unsafe` |
| `crates/ble` | BLE link layer, ATT and the vendor mesh/OTA profile |
| `crates/bulb-core` | the light: curves, colour, persistence, factory reset |
| `crates/bulb-tc32` | the firmware for the chip: start-up, radio drivers, flash |
| `crates/bulb-sim` | host-side model of the bulb for tests |
| `emulator/rs` | TC32/TLSR8258 emulator and the scenario tests (whole bulbs on a simulated air) |
| `crates/outglow` | the `outglow` command: flash a bulb over BLE, read its diagnostics and black box, inspect images |
| `crates/outglow-image` | image formats: ELF to flash image, header and CRC, Zigbee OTA files, boards |
| `xtask` | `cargo xtask`: build the firmware, board tables, OTA index, mutation tests |
| `site` | the website and the web flasher |
| `ota` | the Zigbee2MQTT OTA index |
| `z2m` | Zigbee2MQTT external converter |
| `tools` | stock image extraction from the vendor app, Zigbee2MQTT converter checks |

## Building

The TC32 core of the TLSR8258 has no backend in upstream LLVM, so the build
goes through LLVM bitcode and the [llvm-tc32](https://github.com/goyamamoto/llvm-tc32)
fork:

```sh
export TC32_LLVM=/path/to/llvm-tc32/build/bin
cargo xtask build                              # dimmable filament bulb
cargo xtask build --board EGLO_ZM_TW-4.62      # any board of crates/bulb-tc32/boards.tsv
cargo xtask build --all                        # every board
```

Other tasks: `cargo xtask boards` (the board table from the stock profiles),
`cargo xtask index` (the Zigbee2MQTT OTA index), `cargo xtask mutants`
(mutation tests).

Output in `target/tc32*/`: `bulb-tc32-ota.bin` (BLE update image) and
`bulb-tc32.ota` (Zigbee OTA file).

## Testing

```sh
cargo test --workspace     # the stack and the light on the host
cargo sims                 # whole bulbs in the emulator
cargo spec                 # conformance tests with expectations from the specifications
```

Several scenarios compare Outglow with the original firmware. That firmware
is not in this repository: `tools/extract-stock-images.sh` takes it from the
vendor's Android app on your machine (into `stock/`, which git ignores).
Scenarios whose image is missing are skipped and say where to get it.

## Flashing

1. **First time, over BLE from the original firmware**: the web flasher on
   the website (Chrome or Edge), or from a computer:

   ```sh
   cargo run --release -p outglow -- ble scan
   cargo run --release -p outglow -- ble push <bulb> target/tc32/bulb-tc32-ota.bin --allow-outglow --confirm-write
   ```

   The image must match the bulb's board and hardware revision (the flasher
   checks). Power the bulb off and on just before, and keep it powered.
2. **Afterwards, over Zigbee OTA** from your coordinator (e.g. Zigbee2MQTT).
3. **Going back**: the original firmware can be sent back the same way.

The bootloader of the bulb is the vendor's and is never touched.

## License

GPL-3.0-or-later, see [LICENSE](LICENSE). The emulator core ported from
tc32-devtools stays under the Apache License 2.0, see [NOTICE](NOTICE).

EGLO, AwoX, Telink and Zigbee are trademarks of their respective owners.
Outglow is not affiliated with or endorsed by any of them.
