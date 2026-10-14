# Lyrune GPUI snapshot

This branch starts from Zed `93d745668805029476e6d7fbb19652116fed51de`, the source revision used by `gpui-pre 0.3.9` and `gpui-component 0.7.3`.

It carries the behavior of Lyrune's previous `amtoaer/zed@f029275d696b0a87198e0c6c337712398682fb8a` patches:

- scene damage, unchanged-present skipping, partial WGPU rendering and their existing enabled defaults;
- the runtime inactive-window frame interval setter and unthrottled default;
- transformed glyph painting used by lyrics;
- variable font instances, shaping and rasterization at the requested weight.

The old Wayland demand-driven frame-loop merge is already upstream and is not reapplied. Damage drawing uses the new Wayland frame state machine and preserves renderer recovery, texture bind-group reuse, profiler hooks and headless rendering. Persistent render targets follow the acquired surface dimensions. Offscreen rendering continues to clear its full target using the caller's clear color.

For the component's crates.io GPUI dependencies, use its official `gpui-kit@0.7.3/script/bump-gpui.ts` script with `0.3.9 --zed <this checkout> --stage-only`. The generated standalone workspace preserves crate library names while naming packages `gpui-pre-*`; Lyrune must patch the dependency graph to this single source. Do not publish these packages to crates.io.

Local Linux validation on 2026-10-14:

- Lyrune all-target compile succeeded; application tests: 106 passed, 2 real-account tests ignored.
- WGPU/text/atlas/shader tests: 45 passed, including partial/full pixel equality and a temporary Adwaita Sans variable-font probe.
- Scene-damage tests: 15 passed, including property checks for coverage and disjoint regions. The standalone test harness uses crates.io proptest directly because the Zed property-test wrapper targets its fork.
- Settings cache regression covers scrolling inputs into view and repainting the parent without rebuilding the cached settings page.

The packaging script removes development dependencies. The temporary test harness restores those for WGPU tests and supplies upstream font fixtures; its machine-specific variable-font probe is not part of the published source. Real Wayland window acceptance and macOS/Windows execution are separate from these checks.
