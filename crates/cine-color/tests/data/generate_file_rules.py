#!/usr/bin/env python3
"""File-rule decisions from OpenColorIO, for `tests/file_rules.rs`.

Regenerate:
  PYTHONPATH=<dir containing PyOpenColorIO> python3 generate_file_rules.py > file-rules.tsv
"""
import os

import PyOpenColorIO as ocio

HERE = os.path.dirname(os.path.abspath(__file__))
PATHS = [
    "/job/plates/a001.dpx", "/job/plates/a001.DPX", "/job/Plates/a001.dpx", "/plates/a001.dpx",
    "/job/SHOT_010/comp.exr", "/job/shot_010/comp.exr", "/job/SHOT_010/comp.EYR", "/job/SHOT_010/comp.EXR",
    "shot_12.png", "/x/shot_12.png", "shot_a.png",
    "/work/lin_ap1/beauty.exr", "/work/ACEScct_x/ACEScg.tif", "/work/ACEScc_cg.tif", "/work/cg_ACEScc.tif",
    "/media/clip.mp4", "/media/clip.MOV", "/media/clip.mp4.txt", "/media/clip.mxf",
    "noext", "/a/b.dpx/c",
]
print(f"# OpenColorIO {ocio.__version__}")
print("config\tpath\tcolorspace\trule")
for name in ["file-rules/rules.ocio", "configs/cg-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio"]:
    cfg = ocio.Config.CreateFromFile(os.path.join(HERE, name))
    rules = cfg.getFileRules()
    for p in PATHS:
        cs, index = cfg.getColorSpaceFromFilepath(p)
        print(f"{name}\t{p}\t{cs}\t{rules.getName(index)}")
