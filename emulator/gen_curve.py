#!/usr/bin/env python3
"""CSV from `zigbee_sim.py --sweep` (stock) -> firmware/eglo_curve.h.

  python3 emulator/zigbee_sim.py --bin stock.bin --sweep curve.csv
  python3 emulator/gen_curve.py curve.csv
"""
import csv
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
rows = {int(r["level"]): float(r["duty"]) for r in csv.DictReader(open(sys.argv[1]))}
vals = [0] + [min(65535, round(rows[lv] * 65535 / 100.0)) for lv in range(1, 255)]
body = "\n".join("    " + ", ".join(f"{v:5d}" for v in vals[i:i + 12]) + "," for i in range(0, 255, 12))
out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "..", "firmware", "eglo_curve.h")
hdr = open(out).read().split("static const")[0] if os.path.exists(out) else "#ifndef EGLO_CURVE_H\n#define EGLO_CURVE_H\n"
open(out, "w").write(hdr + "static const unsigned short eglo_level_curve[255] = {\n" + body + "\n};\n#endif\n")
print("written", out)
