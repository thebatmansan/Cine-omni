# Released OpenColorIO ACES configs (test data)

Serialised by PyOpenColorIO 2.5.2 from its built-in configurations:

* `ocio://cg-config-v4.0.0_aces-v2.0_ocio-v2.5`
* `ocio://studio-config-v4.0.0_aces-v2.0_ocio-v2.5`

These configs are produced by the Academy Software Foundation's
OpenColorIO-Config-ACES project and distributed with OpenColorIO under the
BSD-3-Clause licence. They are used here unmodified, only as test input: every display/view pair is
compiled by `cine-color` and its pixels compared with OpenColorIO rendering the
same file (`../config-views.tsv`, from `../generate_config_views.py`;
`tests/released_configs.rs`). Recorded in `licenses/THIRD-PARTY.toml`.
