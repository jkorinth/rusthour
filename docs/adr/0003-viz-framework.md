# ADR-0003: Viewer framework: eframe (egui on wgpu)

- **Status:** accepted
- **Date:** 2026-10-21
- **Decided by:** architect + UX designer (roadmap M1.1)
- **Amended:** 2026-10-22, M1.8: frame-time threshold measured as CPU time per
  frame; browser fetch via `web-sys` instead of `ehttp`

## Context

`rusthour-viz` is the interactive "anthill" viewer. It runs natively and in
the browser (`wasm32-unknown-unknown`) and must:

- draw a lane-level road network (bensheim-5km is the default map) and, from
  M2 on, thousands of moving vehicles per frame, with smooth pan and zoom;
- offer panels, sliders, inspectors and charts (time scale 0.1×–100×+, live
  driver parameters, vehicle inspector, intersection editor, stats overlay);
- let the user click vehicles and intersections;
- step the simulation at a fixed timestep, at a speed independent of the frame
  rate, and later show a remote simulation (M8);
- talk only to `rusthour-api` (ADR-0001), and load maps as `.rhnet` bytes
  decoded by `rusthour-import` (ADR-0002).

The roadmap named Bevy and macroquad. eframe (egui's own app framework on
wgpu) is clearly competitive for a UI-heavy 2D tool and is compared too.

### How the numbers were measured

Versions were checked on crates.io on 2026-10-21. Everything else comes from
a throw-away spike outside the repo: one binary crate per option, with the
dependencies listed in the table header (Bevy with
`default-features = false, features = ["2d", "webgl2", "web"]`, eframe and
macroquad with default features), cargo 1.97.0, on a 2-vCPU Linux sandbox.

- **Crate counts:** unique `name version` pairs in
  `cargo tree -e normal --prefix none`, once for the host
  (`x86_64-unknown-linux-gnu`) and once with
  `--target wasm32-unknown-unknown`.
- **Build time:** clean `cargo check` (dev profile, crates already
  downloaded), wall-clock. A single run, so a rough indication only.
- **Licenses:** `cargo deny check licenses` with this repo's `deny.toml`; the
  origin of each rejected license via `cargo tree -i <crate> --target all`.

The wasm32 target cannot be installed in the sandbox, so wasm binary sizes
were not measured; the "expected wasm size" row is an estimate (see the M1.5
open questions).

## Options considered

| Criterion | Bevy 0.20.0 + bevy_egui 0.43.1 | macroquad 0.4.16 + egui-macroquad 0.17.3 | **eframe 0.37.0** (egui 0.37, wgpu 30) |
| --- | --- | --- | --- |
| Browser backend | wgpu; WebGL2 *or* WebGPU chosen at build time | miniquad's own GL loader (`gl.js`), no wasm-bindgen | wgpu; `webgpu` and `webgl` both enabled by default features, so one build |
| wasm-bindgen ecosystem (HTTP fetch, WebSocket for M8) | yes | no: JS interop via miniquad plugins; `egui-miniquad` pulls `getrandom` 0.2, which expects wasm-bindgen | yes (`ehttp`, `ewebsock`, `web-sys`) |
| Crates in tree (host / wasm32) | 394 / 345 | 96 / 59 | 273 / 110 |
| Clean `cargo check`, host | did not finish: 0.20 refuses rustc 1.97.0 (MSRV 1.97.1); 0.19.1 stopped after 152 s, needing the `libudev` system library | 19 s | 99 s |
| Expected wasm size (unmeasured) | largest | smallest | in between |
| Map and many vehicles | `Mesh2d` + sprite batching; thousands of snapshot entries copied into ECS entities per frame, or a custom instanced material | `draw_*` calls batched automatically; `Camera2D` | epaint `Mesh` built once per network plus per-frame vehicle mesh; wgpu paint callback with instancing as a fallback |
| Panels, sliders, inspectors | bevy_egui (egui 0.36, one release behind) or the young `bevy_ui`/Feathers widgets | egui 0.31 (six releases behind, third-party glue) or macroquad's minimal `ui` | egui 0.37, first-party; charts via egui_plot 0.38 (egui 0.37) |
| Picking | `bevy_picking` (per entity, does not fit instanced vehicles) | none | none built in |
| Driving `Simulation::step` | `Update` system or `FixedUpdate`; sim state lives outside the ECS anyway | plain `loop { …; next_frame().await }` | `App::update` callback; plain struct holds the sim |
| Accessibility | AccessKit on native (`bevy_a11y`), nothing on web | none | AccessKit on native; on web only speech output of the focused widget |
| High-DPI, text, touch | yes, yes, basic | partial, basic, basic | yes, crisp text at any scale, touch and pinch-zoom |
| API stability | breaking release about every 4 months (0.19 June, 0.20 October 2026), across ECS and rendering | 0.4 stable since 2023; small team, 0.5 rewrite in progress | breaking minor release every 2–3 months in 2026 (0.34 March, 0.35 June, 0.36 August, 0.37 October), mostly confined to UI code; MSRV 1.95 |
| Extra licenses vs `deny.toml` | BSL-1.0, MIT-0, OFL-1.1, Ubuntu-font-1.0 | BSL-1.0, OFL-1.1, Ubuntu-font-1.0 | BSL-1.0, OFL-1.1, Ubuntu-font-1.0 |

All three frameworks are `MIT OR Apache-2.0`. The extra licenses come from
transitive dependencies:

- BSL-1.0: `clipboard-win` and its dependency `error-code` (Windows
  clipboard), pulled in by `arboard` (eframe via `egui-winit`, bevy_egui) or
  `copypasta` (egui-miniquad);
- MIT-0: `encase` (Bevy only);
- OFL-1.1 and Ubuntu-font-1.0: `epaint_default_fonts` (any option with egui).

All are compatible with distributing the viewer under GPL-3.0-only. The
fonts are separate works compiled into the binary (`include_bytes!`), which
both font licenses allow, provided their license texts accompany every
redistributed copy (see Consequences).

### Bevy

A full engine: ECS, assets, scenes, 2D/3D rendering, picking, UI, audio. Most
of it does not fit our design: the simulation state lives behind
`rusthour-api`, not in the ECS, so every frame we would copy thousands of
snapshot entries into entities (or bypass the ECS with a custom instanced
material, at which point Bevy adds little). It has the heaviest build and the
largest dependency tree, the highest breaking-change cost, and its egui
integration lags egui by one release. Bevy 0.20 also needs Rust 1.97.1, so
adopting it means a toolchain bump, and its default platform features need
system libraries (`libudev`) on Linux. Its strengths (3D, a scene editor,
`bevy_picking`) are not on our roadmap.

### macroquad

Simple, fast to build, small, and immediate-mode drawing that batches well.
But its browser story is miniquad's own JS loader instead of wasm-bindgen:
fetching `.rhnet` assets, WebSockets for M8 and most web crates need custom
JS plugins. The egui glue is third-party and stuck at egui 0.31, its built-in
UI is too limited for inspectors and editors, and it has no accessibility
support. The upcoming 0.5 rewrite adds migration risk.

### eframe (egui on wgpu)

egui's first-party app framework. Panels, sliders, inspectors, tooltips,
charts (egui_plot) and text are what egui does best, at the current egui
version. On the web it uses wasm-bindgen, with WebGPU and WebGL2 in one
build, and fits the crates we need for asset fetching and M8. The map is
drawn with egui's painter from a mesh built once, so no per-frame path
tessellation; if that turns out too slow, the wgpu backend lets us move the
map and vehicles into our own instanced pipeline without changing framework.

## Decision

`rusthour-viz` uses **eframe 0.37** with the **wgpu** renderer (default
features), on native and in the browser. The browser build is a wasm-bindgen
app built with **Trunk**. Charts use **egui_plot**.

1. **UI:** all panels, sliders, inspectors, charts and overlays are egui.
2. **World rendering starts with egui's painter:**
   - roads and intersections are tessellated **once** per loaded network (or
     per network edit) into an `Arc<epaint::Mesh>` in world coordinates
     (meters); each frame the camera transform (pan/zoom, world → screen) is
     applied and the mesh is painted as `Shape::Mesh`;
   - vehicles (M2.7) are one mesh of quads rebuilt each frame from the
     snapshot;
   - selection highlights, labels and the OSM attribution are drawn with the
     same `Painter`.
3. **Fallback, only if measured too slow:** move roads and vehicles into a
   wgpu pipeline registered via `egui_wgpu::CallbackTrait` (static vertex
   buffer for roads, one instanced draw for vehicles, camera uniform; WGSL
   that runs on WebGL2). The trigger is a missed frame-time threshold:
   - **Threshold:** over 10 s of continuous panning and zooming, the **CPU
     time per frame** (eframe's `frame.info().cpu_usage`: `App::ui`,
     tessellation and render submission, without the vsync wait) has a median
     above **8 ms** or a 95th percentile above **16 ms**; or the **frame
     interval** (`ctx.input(|i| i.unstable_dt)`) has a 95th percentile above
     **1.5× the display's refresh period** (25 ms at 60 Hz). With vsync the
     interval cannot drop below the refresh period, so it only catches
     dropped frames, including GPU-bound ones that CPU time does not show.
   - **Scenes:** bensheim-5km, whole map in view, in M1.5 (map only); and
     bensheim-5km with **5,000 vehicles** in M2.7.
   - **Measure** both with the viewer's `F3` overlay (median and 95th
     percentile by nearest rank over the last 600 frames), natively (release
     build, vsync on) and in the browser (Chrome, release wasm, WebGL2
     forced, since that is the slowest backend). The M1 trial records the
     CPU and interval numbers, the display's refresh rate and the hardware in
     `docs/trials/M1.md`.
   - **Reference setup:** a mid-range laptop with integrated graphics
     (e.g. 4-core x86-64 CPU from 2022 or later, Intel Iris Xe class GPU,
     1920×1080 at 100% scale). The measuring agent records its actual
     hardware in the PR.
   To keep the switch cheap: world coordinates in meters, our own `Camera`
   type, all world drawing behind one small `World` module, and picking that
   never touches egui types.
4. **Picking** is done on the CPU against data, not GPU objects: screen →
   world via the camera, then a search over the snapshot (vehicles) or the
   network (intersections). A linear scan is fine until measured otherwise;
   a uniform grid is the next step. Picking is not part of M1.5. This works
   the same for an in-process or a remote simulation.
5. **Map loading (ADR-0002):** natively the viewer reads the `.rhnet` file;
   in the browser it fetches it with a `web-sys` streaming `fetch`, for
   download progress (`ehttp`'s `streaming` feature would add `futures-util`
   and `wasm-streams`). The
   bytes are decoded with `rusthour-import` (without the `osm` feature) and
   handed to `rusthour-api`; the viewer never touches `rusthour-core`. The app
   is a small state machine: `Loading { progress }` → `Ready(viewer)` or
   `Failed(message)`, so there is no `Simulation` until the map has loaded,
   and the loading and error states are part of the UI (see the UX baseline).
6. **Simulation loop:** the viewer holds a `Box<dyn Simulation>` (or a
   generic) from `rusthour-api`. Pacing is an accumulator fed by egui's frame
   time; the core never reads a clock. When the simulation cannot keep up,
   the viewer shows the speed actually reached (e.g. "running at 37×"): this
   is a requirement, not a nicety.

### How the viewer drives the simulation

```rust
// rusthour-viz; names are illustrative, the API lands in M1.4/M2.6.
struct Viewer {
    sim: Box<dyn rusthour_api::Simulation>,
    time_scale: f64,       // 0.1 ..= 100+, from the speed slider
    running: bool,
    backlog_s: f64,        // simulated seconds owed to the user
    step_cost_s: f64,      // moving average of real time per step
    achieved_scale: f64,   // shown in the UI when below time_scale
    world: World,          // camera + meshes
}

impl eframe::App for Viewer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.running {
            let dt = self.sim.status().dt_s;   // fixed timestep, e.g. 0.1 s
            // Frame time from egui: std::time::Instant panics on wasm32.
            let frame_s = f64::from(ctx.input(|i| i.stable_dt));
            self.backlog_s += frame_s * self.time_scale;
            let wanted = (self.backlog_s / dt).floor() as u64;
            // Time budget, not a fixed step count: step cost grows with
            // the number of vehicles.
            let budget = (STEP_BUDGET_S / self.step_cost_s) as u64;
            let steps = wanted.min(budget);
            let start = web_time::Instant::now(); // viz may read clocks
            self.sim.step(steps);
            self.update_step_cost(start.elapsed(), steps);
            self.backlog_s -= steps as f64 * dt;
            if steps < wanted {
                self.backlog_s = 0.0;           // don't spiral
            }
            self.achieved_scale = steps as f64 * dt / frame_s;
            ctx.request_repaint();
        }
        let snapshot = self.sim.snapshot();    // M2.6: positions, ids, colors
        egui::SidePanel::left("controls").show(ctx, |ui| { /* sliders */ });
        egui::CentralPanel::default().show(ctx, |ui| self.world.show(ui, &snapshot));
    }
}
```

Determinism is unaffected: frame timing only decides *how many* fixed steps
run per frame, never their size. With a remote simulation (M8) the same loop
works if the remote `step`/`snapshot` are non-blocking (the server paces
itself and the viewer shows the latest snapshot); that is a design constraint
for M2.6 and M8.2, not for the viewer.

## M1.5 UX baseline

Checkable criteria for M1.5 and the M1 user trial (AGENTS.md §8). Startup
times use the reference setup above with a warm cache; the trial report also
records the cold-cache time and the wasm and map transfer sizes.

- [ ] **Loading:** the page is never blank or a bare canvas. Before wasm runs,
  the HTML shows a loading text or spinner; while the `.rhnet` downloads, the
  app shows "Loading map… x%".
- [ ] **Failure:** if neither WebGPU nor WebGL2 is available, or the wasm or
  map fails to load, a readable message appears in the page itself (what
  failed, which browser to try), not only in the console.
- [ ] **Startup:** bensheim-5km is *interactive* (pan/zoom responds) within
  **3 s natively** and **5 s in the browser**.
- [ ] **Pan/zoom:** scrolling (mouse wheel or two-finger trackpad scroll)
  zooms around the cursor, as in web maps; trackpad pinch and Ctrl/Cmd+wheel
  zoom too. Drag, arrow keys and WASD pan. (Browsers, and winit on X11 and
  Windows, can't reliably tell a trackpad from a wheel. Panning with
  trackpad scroll on native macOS/Wayland may come later if the M1 trial
  finds this confusing.) Zoom is clamped between "whole map fits" and about
  "one lane fills the screen". `Home` (or a button) resets the view.
- [ ] **Touch** (mobile Chrome and Safari): one finger pans, pinch zooms
  around the pinch center, and the page itself never scrolls or zooms
  (`touch-action: none` on the canvas).
- [ ] **Keyboard:** arrow keys/WASD pan, `+`/`-` zoom; shortcuts don't fire
  while a text field has focus; `?` shows a list of shortcuts. `Space`
  (play/pause) and `[`/`]` (slower/faster) are reserved now and bound in
  M2.7.
- [ ] **Attribution:** "© OpenStreetMap contributors" is visible at every zoom
  level and window size and links to the OSM copyright page.
- [ ] **Text:** a street name with ä, ö, ü and ß renders correctly in a map
  label and in a panel.

### Accessibility minimum

egui draws the web build into a single canvas with no DOM, so **screen
readers cannot navigate the web build**. eframe's `web_screen_reader` feature
(compiled in by default, switched on at runtime) only speaks the focused
widget; we expose it as a settings toggle, off by default. Natively, AccessKit
gives screen readers a real widget tree. We commit to:

- every control reachable and operable by keyboard alone, with a visible
  focus ring;
- every control labelled (this feeds AccessKit and the speech fallback);
- WCAG AA contrast (4.5:1 for text, 3:1 for map features and the selection
  highlight) in both dark and light themes;
- a UI scale setting (egui `zoom_factor`, and browser zoom honoured) that
  works up to 200% without clipped panels;
- never color alone: color coding (from M2.7: vehicle and driver types) also
  has a legend and a shape or pattern.

### Map labels and fonts

Street names are drawn by us with egui's `Painter`, so M1.5 (or a follow-up
item, if M1.5 ships without labels) needs label culling (no overlapping
labels), a minimum on-screen size of about 11 pt logical (smaller labels are
hidden), and a halo or outline for contrast over roads.

egui's bundled fonts cover Latin-1, so German names are fine. There is no
system-font fallback on the web (`system_fonts` is native-only), so glyphs
outside the bundled set show as a box. If we ever import areas outside
Western Europe, ship one font with wide Latin/Cyrillic/Greek coverage.

## Consequences

- One UI toolkit, first-party and current, for the widget-heavy roadmap items
  (M2.7, M4.4, M5.6, M6.5, M7.4).
- M1.5 writes no shaders. A custom wgpu pipeline is added only if the
  frame-time threshold is missed, and the `World`/`Camera` seam keeps that
  change local.
- Picking and the camera are ours and framework-neutral; they can be unit
  tested without a window.
- Moderate dependency weight (110 crates on wasm32, 273 on the host), far
  below Bevy.
- egui ships breaking releases every 2–3 months. We pin minor versions and
  update eframe, egui, egui_plot and wgpu together in one PR; Dependabot
  groups them.
- Web accessibility is limited (a canvas app); native gets AccessKit. The
  accessibility minimum above applies to every viewer PR.
- `deny.toml` needs (in M1.5, when eframe is added):
  - `BSL-1.0` in `allow` (permissive, GPL-compatible);
  - `OFL-1.1` and `Ubuntu-font-1.0` only as an `exceptions` entry for
    `epaint_default_fonts`, not globally.
- Because the default fonts are compiled into the binary, native releases and
  the web build ship third-party notices including the OFL-1.1 and
  Ubuntu-font-1.0 texts (e.g. `OFL.txt`, `UFL.txt`), unless we replace them
  with a single font of our own.
- CI's existing `cargo build --target wasm32-unknown-unknown -p rusthour-viz`
  keeps working; a Trunk build (and a size report) is added in M1.5.
- Rejected for now, revisit only if the roadmap gains 3D or a scene editor:
  Bevy. Rejected: macroquad (no wasm-bindgen, stale egui glue).

## Open questions for M1.5

1. **wasm size and startup:** measure the release build (`wasm-opt`,
   gzip/brotli) and check the startup criteria above. If they are missed, try
   `opt-level = "z"` for the wasm profile and drop unused eframe features.
2. **WebGL2:** confirm eframe runs on the WebGL2 fallback in Firefox and
   Safari and on a mobile browser. (No shaders of our own unless the fallback
   in Decision §3 is triggered.)
3. **Asset discovery:** how the viewer learns the content-hashed asset name
   (ADR-0002), e.g. a small manifest (`maps.json`: area name → hashed file
   name) generated next to the Trunk `copy-file` output, or a fixed index.
   Decide together with M1.6.
4. **Fonts:** keep egui's default fonts (with the license exception and
   notices) or ship one font of our own.
5. **Mobile layout:** on narrow screens the side panel collapses into a
   toggle or bottom sheet so the map keeps most of the viewport; core flows
   avoid text entry (egui's soft-keyboard support on mobile is weak), and
   tooltip content is also reachable by tap-to-select.
6. **Native threading (later):** whether the in-process simulation moves to a
   worker thread natively so the UI stays smooth at 100×; the browser stays
   single-threaded until wasm threads (backlog).

Resolved during review: `getrandom` is not in the wasm32 tree of eframe 0.37
with default features (`cargo tree --target wasm32-unknown-unknown -i
getrandom@0.3.4` and `@0.4.3` print nothing; egui uses `ahash` without
runtime RNG). It appears on native targets only. Recheck when we add our own
web dependencies.
