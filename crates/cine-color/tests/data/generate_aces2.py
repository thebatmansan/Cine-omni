#!/usr/bin/env python3
"""Reference values for the ACES 2.0 output transforms, from OpenColorIO.

The builtins are `ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - <display>_2.0`:
ACES2065-1 in, CIE XYZ out (1.0 = 100 nits). CINE-OMNI's `aces2` module is a
port of the Academy's aces-core CTL; these values are the independent check.

Regenerate:
  PYTHONPATH=<dir containing PyOpenColorIO> python3 generate_aces2.py > aces2-reference.tsv
"""
import random

import PyOpenColorIO as ocio

PREFIX = "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - "
# Every ACES 2.0 output transform OpenColorIO registers.
DISPLAYS = [
    name[len(PREFIX):-len("_2.0")]
    for name, _ in ocio.BuiltinTransformRegistry().getBuiltins()
    if name.startswith(PREFIX) and name.endswith("_2.0")
]
SAMPLES = [
    (0.0, 0.0, 0.0), (0.001, 0.001, 0.001), (0.01, 0.01, 0.01), (0.18, 0.18, 0.18),
    (0.5, 0.5, 0.5), (1.0, 1.0, 1.0), (4.0, 4.0, 4.0), (16.0, 16.0, 16.0),
    (100.0, 100.0, 100.0), (2000.0, 2000.0, 2000.0),
    (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0),
    (1.0, 1.0, 0.0), (0.0, 1.0, 1.0), (1.0, 0.0, 1.0),
    (0.5, 0.1, 0.02), (0.02, 0.3, 0.05), (0.05, 0.08, 0.6), (0.3, 0.18, 0.12),
    (8.0, 0.2, 0.1), (0.1, 6.0, 0.3), (0.2, 0.3, 9.0),
    (-0.05, 0.1, 0.2), (0.3, -0.02, 0.1), (0.2, 0.2, -0.3), (-1.0, -1.0, -1.0),
    (0.18, 0.0, 0.18), (0.0005, 0.02, 0.0001),
]
rng = random.Random(20260916)
for _ in range(20):
    scale = 10 ** rng.uniform(-3, 2)
    SAMPLES.append(tuple(round(scale * rng.uniform(-0.05, 1.0), 6) for _ in range(3)))

cfg = ocio.Config.CreateRaw()
print(f"# OpenColorIO {ocio.__version__}; ACES2065-1 in, CIE XYZ out")
print("display\tdirection\tin0\tin1\tin2\tout0\tout1\tout2")


def cpu(style, direction):
    t = ocio.BuiltinTransform(style)
    t.setDirection(direction)
    return cfg.getProcessor(t).getOptimizedCPUProcessor(
        ocio.BIT_DEPTH_F32, ocio.BIT_DEPTH_F32, ocio.OPTIMIZATION_NONE
    )


for display in DISPLAYS:
    style = f"{PREFIX}{display}_2.0"
    fwd = cpu(style, ocio.TRANSFORM_DIR_FORWARD)
    inv = cpu(style, ocio.TRANSFORM_DIR_INVERSE)
    outputs = []
    for s in SAMPLES:
        r = fwd.applyRGB(list(s))
        outputs.append(r)
        print(f"{display}\tforward\t{s[0]!r}\t{s[1]!r}\t{s[2]!r}\t{r[0]!r}\t{r[1]!r}\t{r[2]!r}")
    # The inverse, on what the forward produced: display colour that exists.
    for s in outputs:
        r = inv.applyRGB(list(s))
        print(f"{display}\tinverse\t{s[0]!r}\t{s[1]!r}\t{s[2]!r}\t{r[0]!r}\t{r[1]!r}\t{r[2]!r}")
