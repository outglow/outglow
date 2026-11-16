# emulator/rs — TLSR8258 emulator and scenario tests

An emulator of the Telink TLSR8258 (TC32 core) with its 802.15.4 and BLE
radios, and on top of it the scenario tests of Outglow: whole bulbs, running
their real firmware image, on a simulated air with a coordinator, other
routers, end devices and a BLE central. The same tests run on the original
firmware of the bulbs, which makes it the reference for behaviour.

Own cargo workspace (`Cargo.toml` here), not a member of the repository-root
one. No external dependencies.

| Crate | What |
|---|---|
| `tc32emu/` | the chip: TC32 CPU, memory map, registers, timers, interrupts, flash, AES, ADC, PWM, sleep and CPU stall. A port of tc32-devtools `go/tc32emu` (Apache-2.0, see NOTICE) |
| `ble/` | the BLE radio model, the air between nodes and a scripted BLE central (link layer, L2CAP, ATT client, SMP legacy and Secure Connections). A port of tc32-devtools `go/ble` (Apache-2.0) |
| `zbnode/` | one emulated bulb: an image on the chip with the 802.15.4 radio model, as a library (`zbnode::Node`) and a binary speaking a line protocol on stdin/stdout |
| `steptrace/` | prints a digest of the machine state every N instructions, to find where two emulator builds diverge |
| `scenarios/` | the tests: conformance suites (Zigbee, BLE) and integration scenarios |

```sh
cargo test                       # the emulator's own tests (tc32emu, ble)
cargo build --release            # target/release/{zbnode,steptrace}
```

## Running the tests

From the repository root (aliases in `.cargo/config.toml`):

```sh
cargo spec                       # Zigbee conformance suite on the Rust image
cargo ble-spec                   # BLE conformance suite on the Rust image
cargo sims                       # integration scenarios (but the weekly ones)
cargo sims-quick                 # a few scenarios for every change
cargo sims-weekly                # the measurements of the original firmware (weekly in CI)
cargo stress                     # the house mesh under loss, noise and power cuts
```

The Rust image is `target/tc32/bulb-tc32.bin` (`cargo xtask build`).
Scenarios that need another image (another board, a second OTA version, the
original firmware) are skipped when it is missing, and say how to get it. The
original firmware comes from the vendor's app on your machine:
`tools/extract-stock-images.sh` (into `stock/`, ignored by git), or from a
checkout of the private repository outglow/original-firmware named by
`OUTGLOW_STOCK_DIR` (its `firmware/<board>.bin`; `stock/stock-dimm.bin` is
`ESMLFzm_w6_Dimm-4.63.bin`, `stock/stock-tw.bin` `ESMLFzm_w6_TW-4.63.bin`,
`stock/apk/smartlightz/<model>__<rev>.bin` `<model>-<rev>.bin`).
`SCENARIOS_IMAGE_DIRS` (separated as `PATH`) lists more directories where
default images are looked for.

### Conformance suites (`cargo spec`, `cargo ble-spec`)

Tests with the clause and requirement of the specification they check (R22,
ZCL 8, BDB 3.0.1, Green Power, Bluetooth Core). A test drives the bulb as a
black box: radio frames, the PWM registers of the light, the BLE central.
Any image works.

```sh
cargo spec -- 'ZCL-ONOFF-01|NWK-RREQ' -v         # some tests (substring of the ID, | = or), observations too
cargo spec -- --skip ZCL-COLOR --cpi 24 --jobs 3 # skip some; faster model; parallel sessions
cargo spec -- --image my=path/to/x.bin@0x6a000   # another image (link offset optional)
cargo spec -- --matrix                           # the original firmware and the Rust image side by side (a report, not a gate)
cargo spec -- --json out.json
cargo spec -- --list
```

The run reports as `cargo test` does: `test rust::ID ... ok`, `... FAILED`
(the failures section gives the clause and what was observed) or
`... ignored, VERDICT: reason` (INCONCLUSIVE, N/A, NOTE: the clause could not
be judged on this image). `--matrix` gives verdicts per image; the original
firmware fails some clauses by design (certified against older revisions).
`-v -v` logs the coordinator's events.

On the default image the run is checked against the committed verdicts in
`scenarios/expect/zigbee-dimm.txt` (`ble-dimm.txt` for `cargo ble-spec`): one
line per test that does not pass, `TEST-ID  VERDICT  reason`; every test not
listed must pass. Any other verdict fails the run, a test that newly passes
too, and so does a run in which a session could not start or nothing passed.
Another image names its file: `--expect zigbee-tw` (`scenarios/expect/NAME.txt`
or a path); `--no-expect` turns the check off. After an intended change
`--write-expect` rewrites the file from the run and leaves `TODO` where a
reason has to be written; the check refuses a file with `TODO` in it.

```sh
cargo spec -- --write-expect                     # the dimmable image's baseline from this run
cargo spec -- --image rust=target/tc32-tw/bulb-tc32.bin --expect zigbee-tw   # the tunable white image
```

Adding a test: a `fn(&mut Session) -> R<String>` in a module under
`scenarios/src/spec/`, registered in its `tests()` with ID, document, clause
and requirement; `expect(cond, msg)?` and `Stop::{Fail, Inconclusive,
NotApplicable}` are the verdicts.

### Integration scenarios (`cargo sims`)

```sh
cargo sims -- --list
cargo sims -- rust_diag rust_staging             # some (substring of the name)
cargo sims -- --skip stress --skip addr_conflict # all but the long ones
cargo sims -- zigbee --image bin=path/to/x.bin   # another image for a role
cargo sims -- rust_fw -v                         # every line as it happens
cargo sims -- --exact rust_fw ota_rust           # these, by their whole name
cargo sims -- --weekly                           # only the weekly ones
cargo sims -- addr_conflict_house                # a weekly one, by its whole name
```

The weekly ones (`WEEKLY` in `scenarios/src/sims/mod.rs`: `addr_conflict`,
`_echo`, `_multihop`, `_house`, `enddev_wallswitch`, `_edinit`)
measure the original firmware for the address conflict study; their checks
hold whatever the numbers, so CI runs them once a week, not every night.

Among them: joining, light and OTA of the Rust firmware (`rust_fw`,
`ota_rust`, `power_cut`), Zigbee and BLE together (`rust_ble`,
`rust_ble_switch`), the first update over BLE from the original firmware
and back (`migrate`, `back_to_stock_ble`, `back_to_stock_ota`), the vendor app's mesh and colour commands (`ble_mesh`, `mesh`,
`interop`, `app_colour`), the parent role under load (`parent_load`),
diagnostics (`rust_diag`, `stress_diag`; the device page's BLE service,
`ble_diag`, with `BLE_DIAG_DUMP=<file>` for the values it read), every board against its original
firmware (`board_check`), frames read by Wireshark (`wireshark`,
`ble_wireshark`, need `tshark`), address conflicts and sleepy end devices
behind a wall switch (`addr_conflict*`, `enddev_wallswitch*`).

Scenarios defined outside this crate can run next to these:
`scenarios::sims::runner::main_with_extra(argv, defs)`.

### Address conflict and wall-switch scenarios

Several bulbs on one air with the coordinator, a scripted router that detects
address conflicts as an EmberZNet router does (R22 3.6.1.9.2; broadcast table
of 9 s / 15 records, relays, Route Reply), and a checker of every
(short, IEEE) pair on the air against the ground truth. Power loss keeps the
flash as it was at that moment.

| Scenario | What |
|---|---|
| `addr_conflict` | a router joined through a bulb changes its address while the bulb is off; probes after power-up |
| `addr_conflict_btt` | the house problem in small: a Device_annce and the coordinator's permit-join 0-20 ms apart, both relayed in time and never late (Rust by default: checked; in the pull request set); `ADDR_CONFLICT_BTT_FULL=1`: how long a bulb remembers a broadcast, Route Request duplicates, Device_annce checks |
| `addr_conflict_echo` | how long one broadcast is relayed in a full mesh |
| `addr_conflict_child` | a neighbouring router changes its address while the bulb is on (Rust by default: checked) |
| `addr_conflict_multihop` | the router reached through two bulbs; route discovery, a power cycle |
| `addr_conflict_house` | nine bulbs, a corridor out of the coordinator's range; `ADDR_CONFLICT_ROUNDS`, `ADDR_CONFLICT_MINUTES` |
| `addr_conflict_victim` | late copies of the bulb's own broadcast: does it change its address? (Rust by default: checked) |
| `addr_conflict_channel` | a channel change while a bulb is off (`CH_WAIT_S`, `CH_DUMP`; Rust by default: checked) |
| `enddev_wallswitch`, `_edinit` | a sleepy end device whose parent bulb is switched off and on: lost presses, rejoin, stale child entries |

Images: `--image bulb=…` (and `--image tw=…` for the tunable white bulbs);
the original firmware by default, but for `addr_conflict_btt`, `_child`,
`_victim` and `_channel`, which run the Rust firmware and check what Outglow does (on
the original firmware the checks of child and channel fail, as measured).
Original bulbs need a valid factory MAC record at 0xFF000 in the image
(BLE MAC little endian, OUI a4:c1:38 last); anything else is replaced by a
"random" one that is the same in every emulated bulb.

### The Rust and the stock firmware together

The stock firmware is AwoX's build of Telink's Zigbee stack: an
implementation of everything the Rust firmware does that owes nothing to
`crates/zigbee`. These scenarios put the two in one network, in either role,
and compare what each does with the same stimulus. Expectations come from the
specification text (R22, ZCL 8, GP Basic 1.1.2, 802.15.4, ZLL); where the
stock firmware does otherwise, the check states the measured stock behaviour
next to the specified one and holds both.

Images: `--image rust=...` (default `target/tc32/bulb-tc32.bin`) and `--image
stock=...` (default `stock/stock-dimm.bin`); for the tunable white pair
`--image rust=target/tc32-tw/bulb-tc32.bin --image stock=stock/stock-tw.bin`.
`cargo sims-stock` runs the quick ones.

| Scenario | What | Wall time |
|---|---|---|
| `mixed_parent_rust`, `mixed_parent_stock` | a bulb that hears only the other joins through it: beacon, association answered on the poll (frame pending), Update-Device, Tunnel, link key, its traffic through the parent; both ways | 15 s |
| `mixed_mesh` | the two relay for each other: unicast both ways, Route Records after many-to-one, source routes, broadcasts relayed once | 50 s |
| `mixed_chain` | coordinator - stock - Rust - stock, each hearing only its neighbours: joins hop by hop, traffic down and up, Route Record with two relays, a source route over both | 25 s |
| `mixed_rejoin` | in that chain the first stock bulb and then the last one lose power: they and the bulbs behind them resume from NV, no association | 60 s |
| `mixed_group` | Add Group, membership, Off/Level/Store/Recall Scene to the group, Remove Group | 10 s |
| `mixed_conflict` | the coordinator gives the stock bulb the address of the Rust bulb while it is off: one address each afterwards, announced, both reachable | 25 s |
| `mixed_ota_relay` | Zigbee OTA of the Rust bulb whose every frame goes through a stock router | 80 s |
| `rust_radio` | the Rust radio driver against the stock firmware's: every RF/DMA/clock register and all analog registers in 802.15.4 receive (each difference with its reason), ACK turnaround, energy scan | 2 s |
| `rust_touchlink` | a scripted initiator against both targets: scan, device information, identify, join router / start network, reset; key index 4 for the stock target (the master key: `TOUCHLINK_MASTER_KEY`, else read from the stock image), 15 or 4 for the Rust one | 40 s |
| `rust_gp` | the two Green Power proxies under the same stimuli: attributes, commissioning notifications, proxy table entries byte for byte, notifications, Commissioning Reply timing | 6 s |
| `migrate` | stock on the network, the Rust update image over the stock BLE OTA, installed, power cycle: the Rust firmware joins again; the stock firmware's NV stays | 3 min |
| `back_to_stock_ota` | the stock image as a `.ota` of the Rust image type over Zigbee OTA from the Rust firmware; installed, the stock firmware joins again | 35 s |
| `back_to_stock_ble` | the stock image over the Rust firmware's BLE OTA; installed, it joins and advertises again | 5 min |

The AwoX bootloader that installs a staged image is not in the emulator
(nobody has it): `transfer::install_staged` does what it is inferred to do
(copy the image staged at 0x6000 and marked 0x4B to 0x6A000). That one step
is not taken from a firmware.

Measured stock behaviour these scenarios hold (each one a check or a note):

- Green Power: an empty proxy table is answered SUCCESS, not NOT_FOUND; the
  shared key defaults to type 0b010 with c0..cf, the cluster revision is 1; a
  frame under an unknown key is dropped, not passed on with its MIC; frames of a
  device paired with its own key (key type 0b100) are not forwarded at all
  (with the shared key they are); the derived alias is stored with the group.
- Touchlink: the stock target offers key index 4 only and puts the ZLL
  profile (0xc05e) in its scan response; it identifies with a breathing light.
- A scene is stored with a blink of the light (new level, old level, off).
- A stock router keeps its parent, the coordinator, as next hop after the link
  to it is gone (4 minutes measured), so it cannot be moved behind another
  router; in `mixed_mesh` it never hears the coordinator.
- After a link loss to its parent of about 20 s, soon after it joined, a stock
  router sometimes gives the parent up for good (Beacon Requests on every
  channel every 22 s, no rejoin through the returned parent, also after its own
  power cycle); not a check, as it does not happen in every run.
- Its BLE events leave BLE values in some radio registers in 802.15.4 mode;
  it acknowledges after 141..144 us (the Rust driver after 161..163 us).
- A BLE update of 156 KB takes 170 s of simulation alone, 660 s while the bulb
  is on the Zigbee network.

In `mixed_chain` (the tunable white pair more than the dimmable one) a frame
of a burst is lost now and then: the Rust relay does not acknowledge a frame
that ends while it prepares a transmission of its own, and the stock sender
gives up after one MAC retry. The coordinator sends without APS
acknowledgement, so the scenario sends an unanswered command once more, as
an APS retry would.

Seen in the Rust firmware and not yet explained: with the third bulb of
`mixed_chain` powered at 24 s (`MIXED_S2_ON=24000`) it sends its Link Status and
0.4 ms later its Device_annce; the Rust bulb's radio takes both, but the Rust
bulb does not pass the announcement on (its receive queue lost nothing). The
default (20 s) does not hit it. `MIXED_DEBUG=1` prints the frames, the radio's
fate of every dropped frame and, for a `--trace` image, the firmware's record.

These scenarios took over from ones that compared the Rust firmware with a C
firmware on Telink's SDK, which is not public:

| Earlier scenario | What it tested | Now |
|---|---|---|
| rust_radio | Rust radio registers against Telink's driver | `rust_radio`, against the stock driver |
| rust_gp | GP proxy next to Telink's | `rust_gp`, next to the stock one |
| rust_mesh | mixed mesh | `mixed_mesh`, `mixed_chain` |
| rust_parent | a Telink-stack bulb joins through the Rust bulb | `mixed_parent_rust`, and the other way `mixed_parent_stock` |
| rust_touchlink | Rust touchlink target next to Telink's | `rust_touchlink`, next to the stock target |
| rust_touchlink_initiator | Telink's initiator (a test build of the C firmware) takes the Rust bulb | not moved: neither stock nor Rust has an initiator; the scripted initiator of `rust_touchlink` does that part |
| migrate | stock -> C firmware by SWS, NV kept, power cycle | `migrate` (stock -> Rust over BLE OTA), `back_to_stock_ota`, `back_to_stock_ble` |
| ota_awox | Zigbee OTA of the C firmware, layout awox | covered by `ota_rust`; Rust -> stock in `back_to_stock_ota` |
| install, install_stock | the C firmware's own bootloader installs an image | not moved: the Rust firmware has no bootloader of its own (layout awox only) |
| sys, sys_ble | supervision of the C firmware (crash counter, safe mode, watchdog, power-on sequence) | covered for Rust by `rust_fw` (watchdog, power cycles, factory reset sequence) and `rust_diag` (reset cause) |
| ble_diag | GATT diagnostic service of the C firmware | `ble_diag`, on the Rust firmware (the same layout, plus the counters characteristic) |
| sense, sense_core | RF motion sensing of the C firmware | not moved here: `rust_sense` runs the Rust firmware's |
| conformance, conformance_clang, stallprobe | the emulator against C code of known behaviour and Telink's own CPU-stall code | not moved: they need Telink's C libraries |

### Stress scenarios (`cargo stress`)

A router-heavy house mesh: emulated bulbs of any images plus scripted
routers ("plugs"), the coordinator as a concentrator (many-to-one every 30 s,
source routes, discover-route on its data frames), traffic to and through the
bulbs, a lossy and noisy channel, rooms switched off at the wall, broadcast
storms.

```sh
cargo stress                                                     # 4 scenarios x stock, rust, rust+stock
cargo stress -- --full --out stress-results.md                   # 9 bulbs
cargo stress -- --sets rust stress_churn --dump /tmp/a.txt
cargo sims -- stress_diag --cpi 24 --image dut=path [--image mix=path]      # one image set (stress_traffic's phases, the Diagnostics counters)
```

Options: `--cpi` (24 by default here), `--jobs` (4), `--seed` (channel model
and traffic; `STRESS_SEED` for `cargo sims`), `--sets` (`stock,rust,rust+stock`;
`a+b`: every second bulb runs `b`), `--stock/--rust PATH` (frozen copies of
the images), `--dump` (every number but wall time, to compare two runs). `STRESS=full`
selects the full house for `cargo sims`. Debugging: `STRESS_DEBUG=n` (failed
messages with their frames, lost relays), `STRESS_TRACE=t0-t1` (every decoded
frame in that span), `STRESS_LIGHT=1`.

`STRESS_ACCOUNT=1` (`=2`: with every frame) accounts, per phase, for each
relayed frame a bulb acknowledged and never passed on, and for each MAC
retransmission: why the attempt before it got no acknowledgement. Sources:
every bulb radio's fate log (each frame that reached the radio and what became
of it: "collision", "off at start: transmitting", "BLE mode", ...) and, for a
Rust image built with `cargo xtask build --trace` (`target/tc32-trace`,
not for real bulbs), the firmware's own record of every frame, flash write and
BLE event.

Fault hooks, off unless a scenario installs them:

- `air::Channel`: per receiver at frame start a `Fate` (`Ok(rssi)`, `Lost`,
  `Corrupt(rssi)`); `chan::ChanModel` implements it from one seeded stream:
  per-link RSSI and loss, a base loss, interference bursts.
- `Node::inject_rx_bad_fcs`: the frame occupies the air and the receiver and
  arrives with a bad FCS.
- `World::power_off` / `power_on`: a bulb without power does not run, send or
  receive; power-on is a reboot at the current air time.
- `Coord::nwk_discover`: the discover-route field of its data frames.

`stress_diag` reads every bulb's Diagnostics counters over ZCL before and after
the traffic phases and holds them against the air (MacTxUcast, retries,
broadcasts against the bulb's own frames; RelayedUcast plus lost relays against
the frames it acknowledged for relaying).

### Tools in `scenarios/examples`

- `device_model -- rust=image.bin`: what a coordinator learns in an interview
  (node descriptor, Basic, endpoints, every attribute) as JSON, the input of
  `tools/z2m-generated-definition.mjs` and `tools/z2m-converter-check.mjs`.
- `stack_depth -- image.bin`: how deep the main and interrupt stacks go while
  a bulb joins. `SIM_STACK=1` makes `rust_ble`, `ble_mesh`, `ota_rust`,
  `rust_staging` and `parent_load` print it too.
- `stock_profile`, `stock_color`: the profile of a board (pins, PWM, curves,
  colour sweeps) taken from an image, as in `analysis/profiles/`.

## Speed and `--cpi`

Nearly all the time goes to the instruction loop: the firmware runs its main
loop flat out, so there is no idle time to skip. One bulb alone runs about
seven times faster than real time on a laptop (10 s simulated in about 1.4 s,
release build); a scenario with nine bulbs is correspondingly slower. The
emulator crates are built optimised even in the dev profile, so
`cargo test` needs no `--release`.

`--cpi` is the number of clock cycles charged per instruction. 3 (the
default) is the chip's; a higher value makes the emulated CPU slower in
simulated time, so fewer instructions are emulated per simulated second and a
run takes less wall time. Firmware with hard deadlines then misses them: at
`--cpi 24` Zigbee logic still behaves the same (the conformance suite gives
the same verdicts), but BLE connection events need `--cpi 3`. Above 24 tests
start to fail for the emulator's sake, not the firmware's.

`cargo run --release -p zbnode --example bench -- image.bin [shift] [ms] [cpi]`
runs one bulb alone and prints the wall time and a digest of the final state
(cycles, registers, SRAM, frames sent): equal digests before and after an
emulator change are a quick check that nothing changed.

## The emulator

### API

`Machine` is a plain struct with public fields (`r`, `regs`, `sram`,
`analog`, `cycles`, `cpu_hz`, `boot_slot`, `asleep`, the flags, `mode`, ...).

- `Flash::new(size)` / `Flash::new_jedec(size, jedec)`; `Machine::new(flash,
  boot_slot, cpi)` owns the flash (`m.flash.mem`, `m.flash.uid`, ...).
  `Machine::with_config(flash, slot, cpi, Config)` takes the settings
  explicitly; `new` reads them from the `TC32EMU_*` environment variables once
  per process.
- `m.reset(boot_slot, cause_wd)`, `m.step()`, `m.run(max_cycles, &[Range],
  check_every)`, `m.run_ms(target_ms, &[Range])`, `m.ms()`, `m.stimer_now()`,
  `m.read/write`, `m.reg_read/reg_write`, `m.take_irq`, `m.irq_pending`,
  `m.pad_levels`, `m.symbolize`, `m.set_symbols`.
- `Error` = `WatchdogReset | SoftwareReset | Emu | Stop | Other`.

### Hooks: `Rc<dyn Fn(&mut Machine, ...)>`

Peripheral models (the radios, the node) are hooks on register and memory
access, time updates, interrupts and resets. Every hook receives the machine;
the machine clones the `Rc` out of its slot and calls it, never emptying the
slot, because models call back into the machine through the same hook chain
(a register write from inside a register write hook goes through the whole
chain again). Chaining:

```rust
let prev = m.reg_write_hook.clone();
m.reg_write_hook = Some(Rc::new(move |m: &mut Machine, o, size, val| {
    if o == 0x602 && size == 1 && val == 0x88 { return Err(stop_err("sram boot")); }
    match &prev { Some(p) => p(m, o, size, val), None => Ok(false) }
}));
```

A hook with state keeps it in `Cell`/`RefCell`; don't hold a borrow across a
call back into the machine. Read hooks return `Result<Option<u32>>`
(`Some(v)`: handled), write hooks `Result<bool>`.

For speed, `mem_read_hook_below` / `mem_write_hook_below` say from which
address on a memory hook does nothing, and `reg_hook_mask` lists the register
bytes the hook chain acts on; any other access skips the chain. **A hook that
starts acting on another register must add it to the mask.**
`ZBNODE_REG_MASK=0` turns the mask off for a comparison run.

### Fidelity

- Integer widths, wrap-around and order of operations follow the reference
  implementation the port came from, so a run is cycle for cycle the same.
  Rust never fuses `x*y + z` on its own; the two places where the reference
  does are written with `mul_add`.
- The radio register writes and their order, the timings of the
  acknowledgement (161–163 µs after a frame) and of BLE events are modelled
  after the chip's vendor driver (its register values and order were compared
  with it).
- CPU stall (Idle mode: 0x80 written to 0x6f): the CPU clock stops, timers,
  watchdog, PWM and the radio go on; an interrupt source in the wake mask at
  `0x78` wakes it. Time jumps to the next event that may wake it (at most
  1 ms per jump).
- Not modelled: analog front-end details beyond what the firmware reads,
  the USB block.

### zbnode

One emulated bulb: the image on the chip, the 802.15.4 radio model
(`radio154.rs`), the BLE radio, the scripted BLE commands (`blespec.rs`) and an
optional register trace (`ZBNODE_REG_TRACE=<file>[,<t0 ms>,<t1 ms>]`). As a
library (`Node::new`, `step_run`, `inject_rx`, `reboot`, `soft_reset`) the
scenarios use it in-process; the binary speaks a line protocol on stdin and
stdout. Opt-in diagnostics on stderr: `ZBNODE_FLASH_LOG`, `_RF_LOG`,
`_ANA_LOG`, `_BLE_LOG`, `_FLASHW_LOG`, `_TRACE`, `_MEMW`, `_PCTRACE_AT`.

### Scenario internals

| Module | What |
|---|---|
| `image.rs` | an image and its link offset (from the ELF next to it, else a prologue heuristic) |
| `air.rs` | the shared 802.15.4 air: conservative lockstep of all nodes, collisions, peers |
| `station.rs` | a scripted node: MAC queue, carrier sense, retries; frame builders where every field can be forced (`Tx`) |
| `coord.rs` | the coordinator and Trust Center (key update and switch, per-key APS counters, permit joining, many-to-one) |
| `peer.rs` | scripted routers with link status, end devices joining through the bulb |
| `frames.rs`, `crypto.rs` | frame parsing and building; AES-CCM*, MMO, HMAC-MMO, SHA-256 |
| `session.rs` | one joined bulb: join, run/wait/collect, ZCL/ZDO requests, the light, a sniffer that learns keys and addresses |
| `runner.rs` | the conformance runner: one joined bulb per (image, session), baseline and alive check between tests |
| `spec/`, `ble/spec/` | the Zigbee and BLE tests |
| `sims/` | the integration scenarios |

The coordinator is deliberately independent of the stack under test: its
crypto is the emulator's AES, its frames are built and parsed here, not by
`crates/zigbee`. A mutation of the firmware's crypto changes only the bulb,
which the scenario then sees.
