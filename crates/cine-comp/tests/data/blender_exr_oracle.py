# Pixel values of an EXR, read by Blender, for `tests/exr_oracle.rs`.
#
# Blender is a separate program run on the command line — nothing of it is
# linked, copied or distributed — and it reads OpenEXR with the Academy's own
# library. That makes it an independent answer to "what is in this file", which
# a round trip through our own reader and writer cannot be.
#
# Regenerate (the file is Blender's own, and is not committed):
#   blender --background --python tests/data/blender_exr_oracle.py -- <file.exr> \
#     | grep -vE "^(Blender |$)" > exr-oracle.tsv
#
# (Blender prints its version banner on exit; the grep keeps the table only.)
import sys

import bpy

path = sys.argv[sys.argv.index("--") + 1]
image = bpy.data.images.load(path)
image.colorspace_settings.name = "Non-Color"
w, h = image.size
pixels = list(image.pixels)
print(f"# Blender {bpy.app.version_string}; {path}")
print(f"# size\t{w}\t{h}")
print("x\ty\tr\tg\tb\ta")
# A deterministic scatter of positions, and the corners.
spots = [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)]
for i in range(40):
    spots.append((i * 977 % w, i * 613 % h))
for x, y in spots:
    # Blender's pixels run bottom-up; EXR rows run top-down.
    row = h - 1 - y
    at = (row * w + x) * 4
    r, g, b, a = pixels[at : at + 4]
    print(f"{x}\t{y}\t{r!r}\t{g!r}\t{b!r}\t{a!r}")
