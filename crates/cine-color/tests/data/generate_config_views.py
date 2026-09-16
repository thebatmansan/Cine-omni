#!/usr/bin/env python3
"""Display/view renderings of the released ACES configs, from OpenColorIO.

Every (display, view) pair of the configs in `configs/`, from ACES2065-1 and
from a video colorspace (the one the configs' own file rules give a movie), on
a fixed set of samples. `tests/released_configs.rs` compiles the same pairs and
compares.

Regenerate:
  PYTHONPATH=<dir containing PyOpenColorIO> python3 generate_config_views.py > config-views.tsv
"""
import os

import PyOpenColorIO as ocio

SAMPLES = [
    (0.0, 0.0, 0.0), (0.02, 0.02, 0.02), (0.18, 0.18, 0.18), (1.0, 1.0, 1.0), (12.0, 12.0, 12.0),
    (0.4, 0.05, 0.02), (0.03, 0.25, 0.04), (0.05, 0.06, 0.5), (0.6, 0.5, 0.1), (-0.02, 0.1, 0.3),
]
HERE = os.path.dirname(os.path.abspath(__file__))
print(f"# OpenColorIO {ocio.__version__}; ACES2065-1 in, display code out")
SOURCES = ["ACES2065-1", "Rec.1886 Rec.709 - Display"]
print("config\tsource\tdisplay\tview\tr\tg\tb\tout_r\tout_g\tout_b")
for file in sorted(os.listdir(os.path.join(HERE, "configs"))):
    if not file.endswith(".ocio"):
        continue
    cfg = ocio.Config.CreateFromFile(os.path.join(HERE, "configs", file))
    for display in cfg.getDisplaysAll():
        views = list(cfg.getViews(ocio.VIEW_DISPLAY_DEFINED, display)) + list(cfg.getViews(ocio.VIEW_SHARED, display))
        for view in views:
            for src in SOURCES:
                t = ocio.DisplayViewTransform(src=src, display=display, view=view)
                try:
                    cpu = cfg.getProcessor(t).getOptimizedCPUProcessor(
                        ocio.BIT_DEPTH_F32, ocio.BIT_DEPTH_F32, ocio.OPTIMIZATION_NONE
                    )
                except Exception as e:  # noqa: BLE001 - recorded, not hidden
                    print(f"# {file} {src} -> {display} / {view}: OpenColorIO refuses: {e}")
                    continue
                for s in SAMPLES:
                    r = cpu.applyRGB(list(s))
                    print(
                        f"{file}\t{src}\t{display}\t{view}\t{s[0]!r}\t{s[1]!r}\t{s[2]!r}"
                        f"\t{r[0]!r}\t{r[1]!r}\t{r[2]!r}"
                    )
