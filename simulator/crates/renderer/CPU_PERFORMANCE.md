# CPU renderer performance audit

Measured November 19, 2026 on Windows x64 (Ryzen 9 9955HX), using the workspace release profile. The baseline is the actual unmodified renderer built with the same synthetic fixture, not the estimates from the reports. Numbers are median milliseconds per operation across seven batches after warm-up; they are not whole-game frame times.

| Workload | Baseline | Updated |
| --- | ---: | ---: |
| Opaque 1:1 SourceOver | 1.2716 | 0.0720 |
| Mixed-alpha 1:1 SourceOver | 3.2728 | 1.6918 |
| Scaled nearest, mask + AddDestination | 41.9103 | 7.0159 |
| Linear rotation, repeated transform | 50.2407 | 2.4251 |
| Linear rotation, changing transform (serial) | 49.2852 | 43.4641 |
| Repeated full text layout with Arial | 0.0501 | 0.0493 via callback / 0.0026 via layout cache |

- **B3/N12**: Opaque 1:1 draws use clipped row copies once immutable pixels have been classified. Axis-aligned nearest draws calculate exact source indices per column and row instead of dividing inside each pixel. Blending writes directly into row slices. The five special modes use integer numerators and exact rounding by 255; the float reference test covers every additive channel/alpha combination and the full channel domain with the relevant alpha branches for the other modes. Affine and bilinear math retain their original floating-point rounding. Triangle scan conversion is in host/runtime crates and is not changed here.
- **N5**: `Canvas::draw_frame` caches sampled pixels for linear scaling and affine draws after the same key is seen twice. The cache is limited to 16 MiB and 16 entries per drawing thread. Source identity, exact transform bits, position, filter and clipped bounds participate in the key. Opacity, mask and blend are applied to the live destination, not cached; list-alpha modes and cheap axis-aligned nearest draws bypass this cache. Animating transforms or changing position can miss. No transform quantization or host-layer policy is used.
- **C2**: Styled glyph masks share coverage and use an 8 MiB byte-budgeted LRU per font (also capped at 2048 entries). The single-font `FontRasterizer::layout_cached` API caches equivalent parsed tokens, style, layout settings and visible prefix, with a separate 2 MiB budget. Mixed-font/callback layout remains uncached; using the layout cache in those host paths requires a separate integration decision.
- **N11**: `Canvas::draw_frame_with_workers` is opt-in. It divides uncached linear affine draws of at least 524,288 destination pixels and 64 rows into disjoint bands; nothing changes layer order. The dedicated fixture measured 45.9523 / 28.5143 / 15.3889 ms with 1 / 2 / 4 workers, with matching hashes and a separate byte-for-byte differential test. The default stays serial; no Android, battery, or host scheduling measurements were taken.

Run the synthetic benchmark from the simulator workspace with `cargo test -p cs2-renderer --release --locked --test cpu_performance -- --include-ignored --nocapture --test-threads=1`. Set `CS2_RENDERER_TEST_FONT` to a local Arial-compatible font path to include the layout fixture. Tests do not use game paths. The golden test folds 66 consecutive frame hashes per blend mode from the original renderer. The pre-existing `hg2_old_and_new_headers_are_inspectable_without_placeholder_pixels` failure is unchanged and outside this work.
