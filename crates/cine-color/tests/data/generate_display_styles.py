#!/usr/bin/env python3
"""Reference values for OCIO display BuiltinTransform styles, from OpenColorIO.

Regenerate:
  PYTHONPATH=<dir containing PyOpenColorIO> python3 generate_display_styles.py > display-styles.tsv
"""
import PyOpenColorIO as ocio

STYLES = [
    "DISPLAY - CIE-XYZ-D65_to_sRGB - MIRROR NEGS",
    "DISPLAY - CIE-XYZ-D65_to_G2.2-REC.709 - MIRROR NEGS",
    "DISPLAY - CIE-XYZ-D65_to_REC.1886-REC.709 - MIRROR NEGS",
    "DISPLAY - CIE-XYZ-D65_to_G2.6-P3-D65 - MIRROR NEGS",
    "DISPLAY - CIE-XYZ-D65_to_DisplayP3",
    "DISPLAY - CIE-XYZ-D65_to_DisplayP3-HDR",
    "DISPLAY - CIE-XYZ-D65_to_ST2084-P3-D65",
]
SAMPLES = [
    (0.0, 0.0, 0.0), (0.18, 0.18, 0.18), (0.9505, 1.0, 1.089), (0.5, 0.2, 0.05),
    (0.1, 0.4, 0.8), (-0.05, 0.1, 0.2), (0.3, -0.02, 0.1), (2.0, 2.1, 2.3),
    (0.001, 0.002, 0.0015), (1.5, 0.3, -0.1),
]
cfg = ocio.Config.CreateRaw()
print(f"# OpenColorIO {ocio.__version__}; CIE-XYZ-D65 in, display code out")
print("style\tx\ty\tz\tr\tg\tb")
for style in STYLES:
    t = ocio.BuiltinTransform(style)
    cpu = cfg.getProcessor(t).getOptimizedCPUProcessor(ocio.BIT_DEPTH_F32, ocio.BIT_DEPTH_F32, ocio.OPTIMIZATION_NONE)
    for s in SAMPLES:
        r = cpu.applyRGB(list(s))
        print(f"{style}\t{s[0]!r}\t{s[1]!r}\t{s[2]!r}\t{r[0]!r}\t{r[1]!r}\t{r[2]!r}")
