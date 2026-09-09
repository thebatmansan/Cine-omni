#!/usr/bin/env python3
"""Generate cine-color's OpenColorIO conformance reference data.

Writes the TSV consumed by crates/cine-color/tests/ocio_conformance.rs.
See the header it emits for the regeneration recipe.
"""
import sys
import PyOpenColorIO as ocio

CONFIG_URI = "ocio://cg-config-v4.0.0_aces-v2.0_ocio-v2.5"

# --------------------------------------------------------------------------
# Sample sets
# --------------------------------------------------------------------------

def linear_samples():
    """Scene-linear: neutral ramp, primaries, secondaries, out-of-gamut,
    near-black, near-white, above 1.0."""
    s = []
    for v in (0.0, 1e-4, 1e-3, 0.01, 0.05, 0.18, 0.5, 0.9, 1.0, 2.0, 8.0, 100.0):
        s.append((f"neutral@{v:g}", (v, v, v)))
    prim = [("red", (1, 0, 0)), ("green", (0, 1, 0)), ("blue", (0, 0, 1)),
            ("cyan", (0, 1, 1)), ("magenta", (1, 0, 1)), ("yellow", (1, 1, 0))]
    for name, p in prim:
        for a in (0.18, 1.0, 4.0):
            s.append((f"{name}@{a:g}", (p[0] * a, p[1] * a, p[2] * a)))
    s += [
        ("oog-neg-r", (-0.1, 0.5, 0.5)),
        ("oog-neg-g", (0.5, -0.1, 0.5)),
        ("oog-neg-b", (0.5, 0.5, -0.1)),
        ("oog-mixed", (-0.05, -0.02, 0.9)),
        ("oog-hot-r", (1.2, -0.3, 0.4)),
        ("near-black-a", (1e-5, 2e-5, 5e-6)),
        ("near-black-b", (1e-3, 5e-4, 2e-3)),
        ("near-black-c", (1e-4, 0.0, 0.0)),
        ("near-white-a", (0.95, 0.98, 1.0)),
        ("near-white-b", (1.0, 1.0, 0.999)),
        ("near-white-c", (0.999, 1.0, 1.001)),
        ("above-one-a", (2.0, 0.5, 0.1)),
        ("above-one-b", (4.0, 3.0, 2.0)),
        ("above-one-c", (16.0, 0.2, 0.05)),
        ("above-one-d", (1000.0, 10.0, 1.0)),
        ("skin-a", (0.18, 0.09, 0.045)),
        ("skin-b", (0.4, 0.2, 0.1)),
        ("foliage", (0.02, 0.3, 0.05)),
    ]
    return s


def code_samples():
    """Code values for log / display encodings: in and slightly beyond [0, 1]."""
    s = []
    for v in (-0.05, -0.001, 0.0, 1e-4, 0.01, 0.05, 0.1, 0.2, 0.3, 0.4, 0.4136,
              0.5, 0.6, 0.7, 0.8, 0.9, 0.95, 0.99, 1.0, 1.001, 1.05):
        s.append((f"code@{v:g}", (v, v, v)))
    s += [
        ("code-red", (0.9, 0.1, 0.1)),
        ("code-green", (0.1, 0.9, 0.1)),
        ("code-blue", (0.1, 0.1, 0.9)),
        ("code-cyan", (0.1, 0.9, 0.9)),
        ("code-magenta", (0.9, 0.1, 0.9)),
        ("code-yellow", (0.9, 0.9, 0.1)),
        ("code-near-black", (0.005, 0.002, 0.008)),
        ("code-near-white", (0.98, 0.995, 1.0)),
        ("code-oog", (-0.02, 0.5, 1.02)),
        ("code-skin", (0.55, 0.45, 0.40)),
    ]
    return s


LINEAR = linear_samples()
CODE = code_samples()
NEUTRAL_CODE = [s for s in CODE if s[1][0] == s[1][1] == s[1][2]]

# --------------------------------------------------------------------------

RAW = ocio.Config.CreateRaw()
CFG = ocio.Config.CreateFromBuiltinConfig(CONFIG_URI)

rows = []
skipped = []


def emit(cpu, group, tid, samples):
    for label, rgb in samples:
        out = cpu.applyRGB(list(rgb))
        rows.append("%s\t%s\t%s\t%.9g %.9g %.9g\t%.9g %.9g %.9g" % (
            group, tid, label, rgb[0], rgb[1], rgb[2], out[0], out[1], out[2]))


def builtin(style, inverse=False):
    t = ocio.BuiltinTransform()
    t.setStyle(style)
    t.setDirection(ocio.TRANSFORM_DIR_INVERSE if inverse else ocio.TRANSFORM_DIR_FORWARD)
    return RAW.getProcessor(t).getDefaultCPUProcessor()


def builtin_chain(styles):
    g = ocio.GroupTransform()
    for style in styles:
        t = ocio.BuiltinTransform()
        t.setStyle(style)
        g.appendTransform(t)
    return RAW.getProcessor(g).getDefaultCPUProcessor()


def run(group, tid, make, samples):
    try:
        emit(make(), group, tid, samples)
    except Exception as e:  # noqa: BLE001 - report and continue, never fabricate
        skipped.append(f"{group}/{tid}: {e}")


# --- transfer functions -----------------------------------------------------
CURVES = [
    ("pq_encode",        "CURVE - LINEAR_to_ST-2084",     False, LINEAR),
    ("pq_decode",        "CURVE - ST-2084_to_LINEAR",     False, CODE),
    ("hlg_encode",       "CURVE - HLG-OETF",              False, LINEAR),
    ("hlg_decode",       "CURVE - HLG-OETF-INVERSE",      False, CODE),
    ("acescct_encode",   "CURVE - ACEScct-LOG_to_LINEAR", True,  LINEAR),
    ("acescct_decode",   "CURVE - ACEScct-LOG_to_LINEAR", False, CODE),
    ("canonlog2_encode", "CURVE - CANON_CLOG2_to_LINEAR", True,  LINEAR),
    ("canonlog2_decode", "CURVE - CANON_CLOG2_to_LINEAR", False, CODE),
    ("canonlog3_encode", "CURVE - CANON_CLOG3_to_LINEAR", True,  LINEAR),
    ("canonlog3_decode", "CURVE - CANON_CLOG3_to_LINEAR", False, CODE),
]
for tid, style, inv, samples in CURVES:
    run("tf", tid, lambda s=style, i=inv: builtin(s, i), samples)

# Display EOTFs isolated on the neutral axis: run the XYZ-D65 -> display
# builtin backwards on code (t,t,t); the Y of the result is the decoded linear
# value (D65 white, Y = 1.0 at code 1.0 in OCIO's 100-nit convention). No
# cine-color matrix is involved, so this measures the curve alone.
DISPLAYS = [
    ("srgb_display_decode_Y",  "DISPLAY - CIE-XYZ-D65_to_sRGB"),
    ("g24_rec709_decode_Y",    "DISPLAY - CIE-XYZ-D65_to_REC.1886-REC.709"),
    ("g22_rec709_decode_Y",    "DISPLAY - CIE-XYZ-D65_to_G2.2-REC.709"),
    ("p3d65_display_decode_Y", "DISPLAY - CIE-XYZ-D65_to_DisplayP3"),
    ("pq_rec2100_decode_Y",    "DISPLAY - CIE-XYZ-D65_to_REC.2100-PQ"),
    ("hlg_rec2100_decode_Y",   "DISPLAY - CIE-XYZ-D65_to_REC.2100-HLG-1000nit"),
]
for tid, style in DISPLAYS:
    run("tf", tid, lambda s=style: builtin(s, True), NEUTRAL_CODE)

# --- primaries / matrix transforms -----------------------------------------
MATRICES = [
    ("ap0_to_xyz_d65",     "UTILITY - ACES-AP0_to_CIE-XYZ-D65_BFD",   False),
    ("ap1_to_xyz_d65",     "UTILITY - ACES-AP1_to_CIE-XYZ-D65_BFD",   False),
    ("ap1_to_lin_rec709",  "UTILITY - ACES-AP1_to_LINEAR-REC709_BFD", False),
    ("lin_rec709_to_ap1",  "UTILITY - ACES-AP1_to_LINEAR-REC709_BFD", True),
    ("acescg_to_aces2065", "ACEScg_to_ACES2065-1",                    False),
    ("aces2065_to_acescg", "ACEScg_to_ACES2065-1",                    True),
]
for tid, style, inv in MATRICES:
    run("mtx", tid, lambda s=style, i=inv: builtin(s, i), LINEAR)

# --- camera IDTs: camera code values -> ACES2065-1 -------------------------
IDTS = [
    ("arri_logc4",             "ARRI_LOGC4_to_ACES2065-1"),
    ("arri_logc3_ei800",       "ARRI_ALEXA-LOGC-EI800-AWG_to_ACES2065-1"),
    ("sony_slog3_sgamut3",     "SONY_SLOG3-SGAMUT3_to_ACES2065-1"),
    ("sony_slog3_sgamut3cine", "SONY_SLOG3-SGAMUT3.CINE_to_ACES2065-1"),
    ("red_log3g10_rwg",        "RED_LOG3G10-RWG_to_ACES2065-1"),
    ("panasonic_vlog_vgamut",  "PANASONIC_VLOG-VGAMUT_to_ACES2065-1"),
    ("canon_clog2_cgamut",     "CANON_CLOG2-CGAMUT_to_ACES2065-1"),
    ("canon_clog3_cgamut",     "CANON_CLOG3-CGAMUT_to_ACES2065-1"),
]
for tid, style in IDTS:
    run("idt", tid, lambda s=style: builtin(s), CODE)

# --- ACES output transforms, ACEScg in, display code values out ------------
def display_view(display, view):
    t = ocio.DisplayViewTransform()
    t.setSrc("ACEScg")
    t.setDisplay(display)
    t.setView(view)
    return CFG.getProcessor(t).getDefaultCPUProcessor()


DVS = [
    ("aces2.0_sdr_srgb",     "sRGB - Display",             "ACES 2.0 - SDR 100 nits (Rec.709)"),
    ("aces2.0_sdr_rec1886",  "Rec.1886 Rec.709 - Display", "ACES 2.0 - SDR 100 nits (Rec.709)"),
    ("aces2.0_sdr_g22",      "Gamma 2.2 Rec.709 - Display","ACES 2.0 - SDR 100 nits (Rec.709)"),
    ("aces2.0_sdr_p3d65",    "Display P3 - Display",       "ACES 2.0 - SDR 100 nits (P3 D65)"),
    ("aces2.0_hdr_pq_rec2020", "Rec.2100-PQ - Display",    "ACES 2.0 - HDR 1000 nits (Rec.2020)"),
]
for tid, disp, view in DVS:
    run("out", tid, lambda d=disp, v=view: display_view(d, v), LINEAR)

# The cg config ships no HLG display, so the Rec.2100-HLG path is built from
# the two builtin styles it would be made of: the ACES 2.0 1000-nit Rec.2020
# output transform, then the HLG display encoding. Input is ACES2065-1 (AP0).
run("out", "aces2.0_hdr_hlg_rec2020_ap0",
    lambda: builtin_chain([
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-1000nit-REC2020_2.0",
        "DISPLAY - CIE-XYZ-D65_to_REC.2100-HLG-1000nit",
    ]), LINEAR)

# ACES 2.0 and ACES 1.0 output transforms in OCIO's XYZ-D65 form (1.0 = 100
# nits) — the form cine-color's ocio.rs::builtin_ops produces for these style
# names. Input is ACES2065-1 (AP0).
run("out", "aces2.0_sdr_rec709_xyz",
    lambda: builtin("ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-100nit-REC709_2.0"), LINEAR)
run("out", "aces1.0_sdr_video_xyz",
    lambda: builtin("ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0"), LINEAR)

# --------------------------------------------------------------------------

header = f"""\
# cine-color colour conformance reference values.
#
# GENERATED — do not hand-edit. Every number below came out of OpenColorIO.
#
#   oracle       OpenColorIO {ocio.GetVersion()} (prebuilt `opencolorio` wheel from PyPI)
#   config       {CONFIG_URI}
#                (reported name: {CFG.getName()}, {len(list(CFG.getColorSpaces()))} colour spaces,
#                 {len(list(CFG.getDisplays()))} displays)
#   generated by tools described below, on the machine recorded in the commit
#
# WHY THIS FILE EXISTS
#   `cine-color` is not ACES-compatible and not OpenColorIO-compatible (see the
#   crate docs and docs/blueprint/research/13-color-management.md). The size of
#   that gap used to be a one-off number in a document. Committing the oracle's
#   answers turns it into a measurement that runs on every change, with no C++
#   dependency in the build. tests/ocio_conformance.rs reads this file.
#
# HOW TO REGENERATE
#   OpenColorIO is NOT required to run the harness — only to regenerate this
#   file. The fastest route needs no C++ toolchain at all:
#
#     uv venv /tmp/ocio-venv
#     uv pip install --python /tmp/ocio-venv/bin/python opencolorio
#     /tmp/ocio-venv/bin/python crates/cine-color/tests/data/generate_reference.py \\
#       > crates/cine-color/tests/data/ocio-reference.tsv
#
#   `generate_reference.py` sits next to this file. Building OpenColorIO from
#   source also works and was attempted first on this machine (zig c++ 0.16 +
#   pip CMake/Ninja, three one-liners: CMAKE_POLICY_VERSION_MINIMUM=3.5,
#   cc/c++/ar/ranlib shims on PATH, and dropping -Wl,--exclude-libs,ALL which
#   zig ld rejects) but on a contended 4-core machine it did not finish in a
#   usable time, and the wheel is the same library.
#
# FORMAT — tab separated, five fields:
#   group  transform-id  sample-label  "in_r in_g in_b"  "ref_r ref_g ref_b"
#
#   group `tf`   1-D transfer functions (OCIO CURVE / DISPLAY builtins).
#                The *_decode_Y rows isolate a display EOTF on the neutral
#                axis: the XYZ-D65 -> display builtin run backwards, whose Y
#                is the decoded linear value. Only the Y (middle) channel is
#                meaningful in those rows.
#   group `mtx`  primaries / matrix transforms (OCIO UTILITY builtins).
#   group `idt`  camera log encodings -> ACES2065-1 (OCIO camera builtins).
#                Inputs are camera code values.
#   group `out`  ACES output transforms. Inputs are ACEScg (AP1 scene-linear)
#                except the ids ending `_xyz` or `_ap0`, whose inputs are
#                ACES2065-1 (AP0), matching the builtin styles' own domain.
#
# Values are printed with %.9g from OCIO's float32 CPU path, so roughly seven
# significant digits are meaningful. Do not read agreement tighter than ~1e-7.
"""

out = sys.stdout
out.write(header)
out.write("#\n# transforms: %d, samples: %d\n" % (
    len({r.split('\t')[1] for r in rows}), len(rows)))
if skipped:
    out.write("# NOT GENERATED (the oracle refused these; they are absent, not zero):\n")
    for s in skipped:
        out.write("#   %s\n" % s)
for r in rows:
    out.write(r + "\n")
sys.stderr.write("rows=%d skipped=%d\n" % (len(rows), len(skipped)))
for s in skipped:
    sys.stderr.write("SKIP %s\n" % s)
