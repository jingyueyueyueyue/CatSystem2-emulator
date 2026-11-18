# Third-Party Notices

The HG pixel-plane/run-length codec, ANM display composition rules, image
window tiling, message marker behavior, glyph coverage/placement, frame
attribute boundary, and resource lifetime contracts in this crate are independent Rust
reimplementations based on CatSystem2 behavior documented and implemented by
the references below. No source file from those projects is linked into the
renderer binary.

- Source: `resources/reference/KatzeSystem2`
- Reference revision: `3184165d03ad9eac0bb6eba3792c4f2f49e07166`
- License: GNU LGPL-2.1-only; see `LICENSE-LGPL-2.1`

- Cross-check source: `resources/reference/FelineSystem2`
- Reference revision: `ce7aeafd7cc6e65763832f7414e4d69d2c9ba48d`
- License status: the checked-in reference tree did not contain a project-wide
  license file; its code is used for behavioral comparison only and is not
  copied into this crate.

The renderer crate is distributed under LGPL-2.1-only. Direct dependencies
retain their upstream terms:

- `fontdue` is distributed under MIT/Apache-2.0 terms.
- `image` is distributed under MIT/Apache-2.0 terms.
- `flate2` is distributed under MIT/Apache-2.0 terms.

The local `LICENSE-LGPL-2.1` file applies to this crate. Reference material
and dependency notices remain separate from the simulator's runtime code.

The renderer's `HgFrameAttributes` and `FrameResourceLease` types are local
interfaces. They preserve decoded `atsNNNN` values and resource ownership
without importing implementation code from the reference projects. Window and
multi-frame composition intentionally clears single-frame HG identity and
attributes because the resulting raster has no unique source frame.
