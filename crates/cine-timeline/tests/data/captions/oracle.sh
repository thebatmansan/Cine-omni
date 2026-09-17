#!/usr/bin/env bash
# Decode each IMSC document with ttconv, an independent TTML reader, into SRT.
#
# ttconv (https://github.com/sandflow/ttconv, BSD-2-Clause) runs as a separate
# program through `uvx`; it is not a dependency of CINE-OMNI and nothing of it
# is linked or shipped. See licenses/THIRD-PARTY.toml.
set -euo pipefail
cd "$(dirname "$0")"
for ttml in *.ttml; do
  out="${ttml%.ttml}.ttconv.srt"
  uvx --from 'ttconv==1.2.3' tt convert -i "$ttml" -o "$out" --itype TTML --otype SRT
  echo "wrote $out"
done
for scc in *.scc; do
  out="${scc}.ttconv.srt"
  uvx --from 'ttconv==1.2.3' tt convert -i "$scc" -o "$out" --itype SCC --otype SRT
  echo "wrote $out"
done
