#!/usr/bin/env python3
"""Board table of the Rust firmware from the stock profiles (docs/stock-profiles.md).

Reads analysis/profiles/*.json (the stock_profile example of emulator/rs/scenarios) and writes
crates/bulb-tc32/src/boards.rs (the table the firmware picks its board from,
BULB_BOARD at build time) and crates/bulb-tc32/boards.tsv (what build.sh needs:
name, tunable, OTA image type). The white bulbs: dimmable (one channel),
tunable white (warm + cold); the colour bulbs: red, green and blue beside
two white channels (RGB+TW), one white of 3000 K (RGB+W), or a white ring
with a colour strip of its own (EBF, docs/stock-colour.md).

  python3 emulator/gen_boards.py            # writes both files
  python3 emulator/gen_boards.py --check    # fails if they are not up to date

OTA image types are fixed here once given: an image type names a board for
good (a server must never hand one board's image to another).
"""
import argparse
import glob
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

# Pins that can carry a PWM output: (pin, mux register, field shift, field
# value, function), from gpio_set_mux() in Telink's platform/chip_8258/gpio.c;
# the same table as PWM_MUX in emulator/rs/scenarios/src/profile.rs.
PWM_MUX = [
    ("PA0", 0x5a8, 0, 1, "PWM0_N"), ("PA2", 0x5a8, 4, 2, "PWM0"), ("PA3", 0x5a8, 6, 2, "PWM1"),
    ("PA4", 0x5a9, 0, 2, "PWM2"), ("PB0", 0x5aa, 0, 0, "PWM3"), ("PB1", 0x5aa, 2, 0, "PWM4"),
    ("PB2", 0x5aa, 4, 0, "PWM5"), ("PB3", 0x5aa, 6, 0, "PWM0_N"), ("PB4", 0x5ab, 0, 1, "PWM4"),
    ("PB5", 0x5ab, 2, 1, "PWM5"), ("PC0", 0x5ac, 0, 1, "PWM4_N"), ("PC1", 0x5ac, 2, 1, "PWM1_N"),
    ("PC1", 0x5ac, 2, 2, "PWM0"), ("PC2", 0x5ac, 4, 0, "PWM0"), ("PC3", 0x5ac, 6, 0, "PWM1"),
    ("PC4", 0x5ad, 0, 0, "PWM2"), ("PC4", 0x5ad, 0, 2, "PWM0_N"), ("PC5", 0x5ad, 2, 0, "PWM3_N"),
    ("PC6", 0x5ad, 4, 2, "PWM4_N"), ("PC7", 0x5ad, 6, 2, "PWM5_N"), ("PD2", 0x5ae, 4, 2, "PWM3"),
    ("PD3", 0x5ae, 6, 0, "PWM1_N"), ("PD4", 0x5af, 0, 2, "PWM2_N"), ("PD5", 0x5af, 2, 0, "PWM0"),
    ("PD5", 0x5af, 2, 2, "PWM0_N"),
]
# GPIO function enable per port (1: plain GPIO, 0: the multiplexed function)
GPIO_FUNC = {"A": 0x586, "B": 0x58e, "C": 0x596, "D": 0x59e, "E": 0x5a6}

# board name -> OTA image type (0x0390/0x0391: the filament bulbs, as before)
IMAGE_TYPES = {
    "ESMLFzm_w6_Dimm-4.63": 0x0390,
    "ESMLFzm_w6_TW-4.63": 0x0391,
    "EGLO_ZM_Dimmable-4.60": 0x0392,
    "EGLO_ZM_Dimmable-4.62": 0x0393,
    "EGLO_ZM_TW-4.60": 0x0394,
    "EGLO_ZM_TW-4.62": 0x0395,
    "EGLO_ZM_TW-4.69": 0x0396,
    "EGLO_ZM_TW_CLP-4.60": 0x0397,
    "ESMLzm_TW_GU10-4.63": 0x0398,
    "EGLO_ZM_RGB_TW-4.60": 0x0399,
    "EGLO_ZM_RGB_TW-4.62": 0x039A,
    "EGLO_ZM_RGB_TW-4.69": 0x039B,
    "EGLO_ZM_RGBTW_CLP-4.60": 0x039C,
    "ESMLzm_c5_E14-4.62": 0x039D,
    "ESMLzm_c5_GU10-4.62": 0x039E,
    "ESMLzm_c9-4.62": 0x039F,
    "ESMLzm_c13g-4.62": 0x03A0,
    "EGLO_ZM_RGB_W-4.62": 0x03A1,
    "EBF_RGB_Zm-4.60": 0x03A2,
    "EBF_RGB_Zm-4.69": 0x03A3,
    "EBF_RGB_Zm_CLP-4.60": 0x03A4,
}


def out_of(pwm, pins):
    """Register fields of the pin that carries PWM channel `pwm`."""
    pin = next(p for p, f in pins if f == f"PWM{pwm}")
    _, mreg, shift, val, _ = next(e for e in PWM_MUX if e[0] == pin and e[4] == f"PWM{pwm}")
    return dict(pin=pin, pwm=pwm, gpio=GPIO_FUNC[pin[1]], bit=1 << int(pin[2]), mux=mreg, shift=shift, value=val)


def board(p):
    name = p["model"] + "-" + p["hw"]
    pins = [tuple(x) for x in p["outputs_on"]["pins"]]
    if any(f.endswith("_N") for _, f in pins):
        raise SystemExit(f"{name}: an inverted PWM output, which the firmware does not drive")
    chans = sorted({int(k[3]) for r in p["level_sweep"] for k in r if k.startswith("pwm")})
    tunable = "ct_sweep" in p
    rgb = None
    if "hue_sweep" in p:
        # the channel at full at hue 0, 84 and 168 (full saturation): red, green, blue
        top = lambda h: [c for c in chans if next(r for r in p["hue_sweep"] if r["v"] == h).get(f"pwm{c}", 0) > 99]  # noqa: E731
        rgb = []
        for h in (0, 84, 168):
            t = [c for c in top(h) if c not in rgb]
            # (EBF: the warm white of the ring stays on beside the strip)
            t = [c for c in t if not all(r.get(f"pwm{c}", 0) > 99 for r in p["hue_sweep"])] or t
            if len(t) != 1:
                raise SystemExit(f"{name}: no single channel at full for hue {h}: {t}")
            rgb.append(t[0])
        chans = [c for c in chans if c not in rgb]
    if rgb and len(chans) == 1:
        # a colour bulb with one white (RGB+W)
        warm, cold = chans[0], None
        ct = (p["color"]["0x400b"], p["color"]["0x400c"])
    elif tunable:
        first, last = p["ct_sweep"][0], p["ct_sweep"][-1]
        cold = [c for c in chans if first.get(f"pwm{c}", 0) > 99 and last.get(f"pwm{c}", 0) < 1]
        warm = [c for c in chans if last.get(f"pwm{c}", 0) > 99 and first.get(f"pwm{c}", 0) < 1]
        if len(chans) != 2 or len(cold) != 1 or len(warm) != 1:
            raise SystemExit(f"{name}: not a warm and a cold channel: {chans}")
        warm, cold = warm[0], cold[0]
        ct = (p["color"]["0x400b"], p["color"]["0x400c"])
    else:
        if len(chans) != 1:
            raise SystemExit(f"{name}: not one channel: {chans}")
        warm, cold, ct = chans[0], None, (153, 454)
    periods = {p["outputs_on"]["chans"][c]["max"] for c in chans + (rgb or [])}
    if len(periods) != 1:
        raise SystemExit(f"{name}: channels with different periods {periods}")
    beside = p["model"].startswith("EBF")
    kind = ("open-eglo-ebf" if beside else "open-eglo-rgbtw" if cold is not None else "open-eglo-rgbw") if rgb else \
        ("open-eglo-tw" if cold is not None else "open-eglo-dimm")
    return dict(name=name, model=p["model"], product=int(p["ble_product"][0], 16), hw_revision=p["hw"],
                hw_version=int(p["hw"].split(".")[1]), warm=out_of(warm, pins),
                cold=out_of(cold, pins) if cold is not None else None,
                rgb=[out_of(c, pins) for c in rgb] if rgb else None, beside=beside,
                colour_default=(160, 254) if beside else (170, 254), kind=kind, period=periods.pop(),
                ct_min=ct[0], ct_max=ct[1], image_type=IMAGE_TYPES[name])


def rs_out(o):
    return (f"Out {{ pwm: {o['pwm']}, gpio: {o['gpio']:#05x}, bit: {o['bit']:#04x}, mux: {o['mux']:#05x}, "
            f"shift: {o['shift']}, value: {o['value']} }}")


def render(boards):
    rs = ["//! The boards the firmware is built for (`BULB_BOARD`, build.sh `BOARD=`): generated by",
          "//! emulator/gen_boards.py from the stock firmware's own behaviour in the emulator",
          "//! (analysis/profiles/, docs/stock-profiles.md). Do not edit; run the generator.",
          "",
          "use crate::board::{Board, Out};",
          "",
          f"pub const BOARDS: [Board; {len(boards)}] = ["]
    for b in boards:
        rgb = f", {'/'.join(o['pin'] for o in b['rgb'])} red/green/blue" if b["rgb"] else ""
        rs += [f"    // {b['warm']['pin']}" + (f" warm, {b['cold']['pin']} cold" if b["cold"] else "") + rgb,
               "    Board {",
               f"        name: \"{b['name']}\",",
               f"        tag: \"open-eglo-board:{b['name']}\",",
               f"        model: \"{b['model']}\",",
               f"        ble_product: {b['product']:#06x},",
               f"        hw_revision: \"{b['hw_revision']}\",",
               f"        hw_version: {b['hw_version']},",
               f"        warm: {rs_out(b['warm'])},",
               f"        cold: {'Some(' + rs_out(b['cold']) + ')' if b['cold'] else 'None'},",
               f"        rgb: {'Some([' + ', '.join(rs_out(o) for o in b['rgb']) + '])' if b['rgb'] else 'None'},",
               f"        rgb_beside_white: {'true' if b['beside'] else 'false'},",
               f"        colour_default: ({b['colour_default'][0]}, {b['colour_default'][1]}),",
               f"        kind: \"{b['kind']}\",",
               f"        period: {b['period']},",
               f"        ct_min: {b['ct_min']},",
               f"        ct_max: {b['ct_max']},",
               f"        image_type: {b['image_type']:#06x},",
               "    },"]
    rs += ["];", ""]
    tsv = ["# name\ttunable\timage_type (generated by emulator/gen_boards.py)"]
    tsv += [f"{b['name']}\t{1 if b['cold'] or b['rgb'] else 0}\t{b['image_type']:#06x}" for b in boards]
    return "\n".join(rs), "\n".join(tsv) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--profiles", default=os.path.join(ROOT, "analysis", "profiles"))
    ap.add_argument("--check", action="store_true")
    a = ap.parse_args()
    found = {}
    for f in sorted(glob.glob(os.path.join(a.profiles, "*.json"))):
        p = json.load(open(f))
        name = f"{p['model']}-{p['hw']}"
        if name in IMAGE_TYPES and p.get("joined"):
            found[name] = board(p)
    missing = set(IMAGE_TYPES) - set(found)
    if missing:
        raise SystemExit(f"no profile for {sorted(missing)}")
    boards = [found[n] for n in IMAGE_TYPES]
    rs, tsv = render(boards)
    targets = {os.path.join(ROOT, "crates", "bulb-tc32", "src", "boards.rs"): rs,
               os.path.join(ROOT, "crates", "bulb-tc32", "boards.tsv"): tsv}
    if a.check:
        stale = [p for p, t in targets.items() if not os.path.exists(p) or open(p).read() != t]
        if stale:
            raise SystemExit(f"out of date: {stale}")
        print("boards up to date")
        return
    for p, t in targets.items():
        open(p, "w").write(t)
        print("written", p)


if __name__ == "__main__":
    main()
