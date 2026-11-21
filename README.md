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
  and attempts). A [Zigbee2MQTT converter](z2m/outglow.mjs) shows them.
- **Steadier routing**: link cost from measured delivery with smoothing and
  hysteresis, so routes do not flap.
- **Bluetooth LE kept**: the bulb still speaks the vendor app's profile, so the
  vendor app and the BLE update path keep working; BLE can be switched off or
  limited to a window after power-on from Zigbee.
- **Motion sensing (experimental, off until switched on)**: the bulbs can
  notice people moving from how the radio signal between them changes, and
  report it as occupancy. Switch it on in at least two bulbs in a room (the
  converter's `motion_sensing`), then let it calibrate itself for a day, or
  calibrate it by hand with the room empty. In the emulator only so far, not
  yet run on a real bulb.
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
fork. `cargo xtask toolchain` downloads a build of it from this repository's
releases; elsewhere, build it yourself and set `TC32_LLVM` to its `bin`.
Build in the Nix shell of `flake.nix` (`nix develop`), as CI does: rustc's
LLVM shapes the code, so only that pinned rustc gives the same image byte for
byte (`cargo xtask build` warns under any other).

```sh
nix develop                                    # the pinned rustc, cargo and std sources
cargo xtask toolchain                          # downloads llvm-tc32 (Linux x86-64, macOS arm64)
cargo xtask build                              # dimmable filament bulb
cargo xtask build --board EGLO_ZM_TW-4.62      # any board of crates/bulb-tc32/boards.tsv
cargo xtask build --all                        # every board
cargo xtask build --no-sense                   # without motion sensing (target/tc32-nosense)
```

Other tasks: `cargo xtask boards` (the board table from the stock profiles),
`cargo xtask index` (the Zigbee2MQTT OTA index), `cargo xtask coverage`
and `cargo xtask mutants` (coverage and mutation tests, under Testing).

Output in `target/tc32*/`: `bulb-tc32-ota.bin` (BLE update image) and
`bulb-tc32.ota` (Zigbee OTA file). Only `bulb-tc32` reads the board, so the
crates under it (`core`, `zigbee`, `ble`, ...) compile once per set of
features into `target/tc32-cargo/`, for every board; `--all` then compiles
`bulb-tc32` for the other boards side by side, each in a copy of that
directory.

## Testing

```sh
cargo test --workspace     # the stack and the light on the host
cargo sims                 # whole bulbs in the emulator, all on Outglow
cargo spec                 # conformance tests with expectations from the specifications
```

Several scenarios compare Outglow with the original firmware (`cargo
sims-stock`, or `cargo sims -- --firmware stock` for the mixed networks too;
by default every bulb runs Outglow). That firmware is not in this repository: `tools/extract-stock-images.sh` takes it from the
vendor's Android app on your machine (into `stock/`, which git ignores).
Scenarios whose image is missing are skipped and say where to get it.

### Coverage and mutation testing

Line and region coverage of the host tests (`cargo test --workspace`, and
with `--emulator` the emulator's unit tests) comes from
[cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) in a shell of
its own, `nix develop .#coverage`: the official Rust binaries of the same
version, whose llvm-tools read the profiles their compiler writes (nixpkgs'
rustc writes profiles its own LLVM cannot read, and its instrumented tests
crash on macOS). That shell is only for coverage; images are built in
`nix develop`.

```sh
nix develop .#coverage --command cargo xtask coverage             # table per crate, worst files, HTML
nix develop .#coverage --command cargo xtask coverage --check     # and fail below xtask/coverage.tsv
nix develop .#coverage --command cargo xtask coverage --update    # raise the baseline after adding tests
```

The HTML report is in `target/coverage/host/html/index.html`. CI runs
`--emulator --check` on every push and pull request: the table goes to the
job summary, the HTML report is the artifact `coverage-html`, and a crate
whose line or region coverage falls below its baseline in
`xtask/coverage.tsv` fails the job (a ratchet: the baseline only moves up,
with `--update`).

`crates/bulb-tc32` (its own workspace) is measured too: on the host its
register accesses go to a simulated chip (`src/sim.rs`: a logged register
file with the timer, analog bus, MSPI flash, AES engine, RF status and the
BLE TX fifo the drivers wait for; a test can hook every access to play the
other side, and a test that runs 10 s is stopped), and its tests
(`src/tests/`) check the drivers' register writes, their order and values
against the vendor's traced write lists, their timing to the tick, and the
whole bulb: the main loop, and a BLE central on the simulated radio that
connects, reads the device page and logs in like the app. On the chip the
same `reg.rs` primitives are volatile MMIO. What only the emulator reaches:
`boot/start.S` (the assembly start-up before `main`), the linker's memory
layout (`.ram_code` placement, `.noinit`, the stack measured by
`stack.rs`) and real time (interrupt latency, what the radio does on air).
Its mutants run with every feature (`all_features` in its
`.cargo/mutants.toml`), so the trace code is mutated too.

```sh
cargo test --manifest-path crates/bulb-tc32/Cargo.toml      # the drivers on the simulated chip
cargo mutants --manifest-path crates/bulb-tc32/Cargo.toml -j 2
```

Mutation testing: [cargo-mutants](https://mutants.rs) (in `nix develop`;
config in `.cargo/mutants.toml`) changes the code one small step at a time
and checks that the tests of the crate notice.

```sh
cargo mutants -p sense -j 2                       # every mutant of a crate; results in mutants.out/
cargo mutants --in-diff <(git diff main) -j 2     # only the lines a change touches
cargo xtask mutants-report mutants.out            # the table per crate, the missed mutants
cargo xtask mutants                               # the hand-written mutations of xtask/src/mutants/
```

On a pull request CI runs the mutants of the changed lines (workflow
`mutants.yml`; a missed one fails it). Every mutant of the workspace runs
weekly in 16 parts, with the table per crate in the summary of the job
`report`. The hand-written mutations of `cargo xtask mutants` stay: they
break behaviour the specifications describe (a counter not counted, a frame
not relayed, a timeout of the wrong length) across whole statements and
constants, which cargo-mutants' operator and return-value mutants do not
reach. The test infrastructure (bulb-sim, memcap) is not mutated, nor is
the outglow command's glue to the Bluetooth adapter and the MQTT broker
(`flash/bt.rs`, `diag/mqtt.rs`, left out of the coverage too: it needs a
bulb or a broker; the logic behind it runs on fakes in the tests), and
bulb-core's mutants run bulb-sim's tests too:

```sh
cargo mutants -p bulb-core --test-package bulb-core,bulb-sim --timeout 120 -j 2
```

Every host test binary runs on memcap's capped allocator (4 GiB,
`OUTGLOW_TEST_MEM_LIMIT_MB`): a mutant that makes a test allocate without
end stops there; bulb-tc32's tests (another workspace, which cargo-mutants
copies without memcap) stop at a million register accesses in one test, or
after 10 s.
Locally, keep `-j` at 2 on a laptop.

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
