#!/usr/bin/env python3
"""Colorspace conversions of the released ACES configs, from OpenColorIO.

For every colorspace of the configs in `configs/` (inactive included), the
conversion from ACES2065-1 to it and from it back to ACES2065-1, on a fixed set
of samples. `tests/released_configs.rs` compiles the same conversions and
compares.

Regenerate:
  PYTHONPATH=<dir containing PyOpenColorIO> python3 generate_config_colorspaces.py > config-colorspaces.tsv
"""
import os

import PyOpenColorIO as ocio

SAMPLES = [
    (0.0, 0.0, 0.0), (0.02, 0.02, 0.02), (0.18, 0.18, 0.18), (1.0, 1.0, 1.0),
    (0.4, 0.05, 0.02), (0.03, 0.25, 0.04), (0.05, 0.06, 0.5), (0.35, 0.3, 0.2),
]
HERE = os.path.dirname(os.path.abspath(__file__))
print(f"# OpenColorIO {ocio.__version__}")
print("config\tsrc\tdst\tr\tg\tb\tout_r\tout_g\tout_b")
for file in sorted(os.listdir(os.path.join(HERE, "configs"))):
    if not file.endswith(".ocio"):
        continue
    cfg = ocio.Config.CreateFromFile(os.path.join(HERE, "configs", file))
    names = cfg.getColorSpaceNames(ocio.SEARCH_REFERENCE_SPACE_ALL, ocio.COLORSPACE_ALL)
    for name in names:
        cs = cfg.getColorSpace(name)
        if cs.isData():
            continue
        for src, dst in (("ACES2065-1", name), (name, "ACES2065-1")):
            if src == dst:
                continue
            try:
                cpu = cfg.getProcessor(src, dst).getOptimizedCPUProcessor(
                    ocio.BIT_DEPTH_F32, ocio.BIT_DEPTH_F32, ocio.OPTIMIZATION_NONE
                )
            except Exception as e:  # noqa: BLE001 - recorded, not hidden
                print(f"# {file} {src} -> {dst}: OpenColorIO refuses: {e}")
                continue
            # From ACES2065-1: scene samples. Into it: the forward result, so a
            # display colorspace is fed display colour that exists.
            inputs = SAMPLES
            if dst == "ACES2065-1":
                fwd = cfg.getProcessor("ACES2065-1", src).getOptimizedCPUProcessor(
                    ocio.BIT_DEPTH_F32, ocio.BIT_DEPTH_F32, ocio.OPTIMIZATION_NONE
                )
                inputs = [tuple(fwd.applyRGB(list(s))) for s in SAMPLES]
            for s in inputs:
                r = cpu.applyRGB(list(s))
                print(f"{file}\t{src}\t{dst}\t{s[0]!r}\t{s[1]!r}\t{s[2]!r}\t{r[0]!r}\t{r[1]!r}\t{r[2]!r}")
