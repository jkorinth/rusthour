//! Rust Hour interactive viewer, native and in the browser (ADR-0003).

mod camera;
mod demo;
mod frame_stats;
mod labels;
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, reason = "used by the browser's loader, tested natively")
)]
mod manifest;
mod map;
#[cfg(target_arch = "wasm32")]
mod web;
mod world;

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align2, Key};
use rusthour_api::{Config, InProcess, Simulation};

use frame_stats::FrameTimes;
use map::MapSource;
use world::World;

/// What a loader hands to the app.
pub enum Loaded {
    /// `.rhnet` bytes.
    Map(Vec<u8>),
    /// Show the demo network, with a note for the user saying why, if any.
    Demo(Option<String>),
    /// Loading failed; the message is for the user.
    Failed(String),
}

/// Download state, shared between the browser's fetch and the app.
#[derive(Default)]
pub struct Progress {
    /// Area being downloaded, once known.
    pub area: String,
    /// Bytes received so far.
    pub received: u64,
    /// Expected size, `0` if unknown.
    pub total: u64,
    /// Areas on this site the user can switch to, from `maps/manifest.json`.
    pub maps: Vec<String>,
    /// Set when the loader is done.
    pub result: Option<Loaded>,
    /// "Preparing map…" has been shown: decoding blocks the (browser's only)
    /// thread, so the text goes up one frame before it starts.
    pub preparing: bool,
}

/// ADR-0003 §5: Loading → Ready | Failed.
enum State {
    #[cfg_attr(
        not(target_arch = "wasm32"),
        allow(dead_code, reason = "only the browser loads asynchronously")
    )]
    Loading(Arc<Mutex<Progress>>),
    Ready(Box<Viewer>),
    Failed(String),
}

impl State {
    fn from_loaded(loaded: Loaded) -> Self {
        let viewer = match loaded {
            Loaded::Map(bytes) => map::from_rhnet(&bytes).and_then(|m| Viewer::new(m, None)),
            Loaded::Demo(note) => Viewer::new(demo::map(), note),
            Loaded::Failed(message) => Err(message),
        };
        match viewer {
            Ok(v) => Self::Ready(Box::new(v)),
            Err(message) => Self::Failed(message),
        }
    }

    /// Takes the loader's result, keeping the map list for the selector.
    fn finish(p: &mut Progress) -> Self {
        let mut state = Self::from_loaded(p.result.take().unwrap_or(Loaded::Demo(None)));
        if let Self::Ready(v) = &mut state {
            v.maps = std::mem::take(&mut p.maps);
        }
        state
    }
}

/// A loaded map and its view.
struct Viewer {
    world: World,
    attribution: String,
    attribution_url: Option<&'static str>,
    source_date: Option<String>,
    /// Why the demo network is shown, until dismissed.
    note: Option<String>,
    /// For the debug overlay: area name, nodes, roads.
    stats: (String, usize, usize),
    /// Areas the map selector offers; empty hides it.
    maps: Vec<String>,
}

impl Viewer {
    fn new(map: MapSource, note: Option<String>) -> Result<Self, String> {
        let sim = InProcess::with_network(Config::default(), 0, map.network)
            .map_err(|e| format!("The map “{}” is not a valid network: {e}.", map.name))?;
        let net = sim.network();
        Ok(Self {
            stats: (map.name, net.nodes.len(), net.roads.len()),
            world: World::new(net),
            attribution: map.attribution,
            attribution_url: map.attribution_url,
            source_date: map.source_date,
            note,
            maps: Vec::new(),
        })
    }
}

/// Starts loading the map, again on "Retry".
type Load = Box<dyn Fn() -> State>;

/// The `F3` overlay: frame times and startup time, checked against
/// ADR-0003's thresholds in the M1 trial.
#[derive(Default)]
struct Debug {
    shown: bool,
    /// Time between frames (egui's `unstable_dt`), capped by vsync.
    interval: FrameTimes,
    /// CPU time per frame (UI and rendering, without waiting for vsync).
    cpu: FrameTimes,
    /// Seconds from start to the first frame with a map.
    startup_s: Option<f64>,
}

impl Debug {
    /// Shows or hides the overlay, with fresh samples: the first interval
    /// would include the idle time before.
    fn toggle(&mut self) {
        *self = Self {
            shown: !self.shown,
            startup_s: self.startup_s,
            ..Self::default()
        };
    }
}

struct App {
    state: State,
    load: Load,
    show_help: bool,
    debug: Debug,
}

impl App {
    fn new(state: State, load: Load) -> Self {
        Self {
            state,
            load,
            show_help: false,
            debug: Debug::default(),
        }
    }
}

/// What the user chose on the Failed screen.
enum FailedAction {
    Retry,
    Demo,
}

/// Native: since `main` started.
#[cfg(not(target_arch = "wasm32"))]
static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Seconds since `main` started.
#[cfg(not(target_arch = "wasm32"))]
fn seconds_since_start(_ctx: &egui::Context) -> f64 {
    START.get().map_or(0.0, |t| t.elapsed().as_secs_f64())
}

/// Seconds since the page started loading: egui's time is
/// `performance.now()` in the browser.
#[cfg(target_arch = "wasm32")]
fn seconds_since_start(ctx: &egui::Context) -> f64 {
    ctx.input(|i| i.time)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let State::Loading(progress) = &self.state {
            let mut p = progress.lock().expect("not poisoned");
            if p.result.is_some() && !p.preparing {
                p.preparing = true;
            } else if p.result.is_some() {
                let state = State::finish(&mut p);
                drop(p);
                self.state = state;
            }
        }
        match &mut self.state {
            State::Loading(progress) => {
                let p = progress.lock().expect("not poisoned");
                show_loading(ui, &p);
                // The fetch can't wake egui up.
                ctx.request_repaint();
            }
            State::Failed(message) => match show_failed(ui, message) {
                Some(FailedAction::Retry) => self.state = (self.load)(),
                Some(FailedAction::Demo) => self.state = State::from_loaded(Loaded::Demo(None)),
                None => {}
            },
            State::Ready(viewer) => {
                let d = &mut self.debug;
                d.startup_s.get_or_insert_with(|| seconds_since_start(&ctx));
                if !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::F3)) {
                    d.toggle();
                }
                if d.shown {
                    d.interval.push(ctx.input(|i| i.unstable_dt));
                    d.cpu.extend(frame.info().cpu_usage);
                    // Measure every frame, not only frames with input.
                    ctx.request_repaint();
                }
                let chosen = show_ready(ui, viewer, &mut self.show_help, d);
                #[cfg(target_arch = "wasm32")]
                if let Some(area) = chosen {
                    web::open_map(&area);
                }
                #[cfg(not(target_arch = "wasm32"))]
                let _ = chosen; // Native has no map list (yet).
                if d.shown {
                    show_debug(&ctx, d, viewer);
                }
            }
        }
    }
}

fn show_debug(ctx: &egui::Context, d: &Debug, v: &Viewer) {
    let ms = |t: &FrameTimes| match t.median_p95() {
        Some((median, p95)) => format!(
            "median {:.1} ms, p95 {:.1} ms ({} frames)",
            median * 1e3,
            p95 * 1e3,
            t.len()
        ),
        None => "no frames yet".to_owned(),
    };
    egui::Area::new(egui::Id::unique("debug"))
        .anchor(Align2::LEFT_BOTTOM, [8.0, -44.0])
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                egui::Grid::new("debug_grid").show(ui, |ui| {
                    let (name, nodes, roads) = &v.stats;
                    let rows = [
                        ("Frame interval", ms(&d.interval)),
                        ("CPU per frame", ms(&d.cpu)),
                        (
                            "Startup",
                            d.startup_s.map_or_else(String::new, |s| {
                                format!("{s:.2} s to first map frame")
                            }),
                        ),
                        (
                            "Map",
                            format!(
                                "{name}: {nodes} nodes, {roads} roads, {} named",
                                v.world.labels.len()
                            ),
                        ),
                    ];
                    for (what, value) in rows {
                        ui.label(what);
                        ui.label(value);
                        ui.end_row();
                    }
                });
                ui.small(format!(
                    "Last {} s at 60 Hz. ADR-0003: CPU per frame median ≤ 8 ms, p95 ≤ 16 ms; \
                     frame interval p95 ≤ 1.5× refresh period.",
                    frame_stats::WINDOW / 60
                ));
            });
        });
}

/// A centered column about 60 characters wide, a third down the window.
fn centered(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::CentralPanel::default().show(ui, |ui| {
        let full = ui.available_rect_before_wrap();
        let width = full.width().min(480.0);
        let column = egui::Rect::from_x_y_ranges(
            full.center().x - width / 2.0..=full.center().x + width / 2.0,
            full.top() + full.height() / 3.0..=full.bottom(),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
            ui.vertical_centered(add);
        });
    });
}

fn show_loading(ui: &mut egui::Ui, p: &Progress) {
    centered(ui, |ui| {
        let what = if p.area.is_empty() {
            "map".to_owned()
        } else {
            format!("map “{}”", p.area)
        };
        if p.preparing || (p.total > 0 && p.received >= p.total) {
            ui.label(format!("Preparing {what}…"));
            ui.spinner();
        } else if p.total > 0 {
            #[expect(clippy::cast_precision_loss, reason = "a progress bar")]
            let fraction = (p.received as f32 / p.total as f32).min(1.0);
            ui.label(format!("Loading {what}… {:.0}%", fraction * 100.0));
            ui.add(egui::ProgressBar::new(fraction).desired_width(240.0));
        } else {
            ui.label(format!("Loading {what}…"));
            ui.spinner();
        }
    });
}

/// The Failed screen; returns the button clicked, if any.
fn show_failed(ui: &mut egui::Ui, message: &str) -> Option<FailedAction> {
    let mut action = None;
    centered(ui, |ui| {
        let strong = ui.visuals().strong_text_color();
        ui.label(
            egui::RichText::new("The map could not be loaded")
                .size(22.0)
                .color(strong),
        );
        ui.add_space(8.0);
        ui.label(egui::RichText::new(message).size(16.0).color(strong));
        ui.add_space(12.0);
        if ui.button("Retry").clicked() {
            action = Some(FailedAction::Retry);
        }
        if ui.button("Show demo map").clicked() {
            action = Some(FailedAction::Demo);
        }
    });
    action
}

/// The map view and its controls; returns the area picked in the map
/// selector, if the user switched maps.
fn show_ready(
    ui: &mut egui::Ui,
    v: &mut Viewer,
    show_help: &mut bool,
    debug: &mut Debug,
) -> Option<String> {
    let mut chosen = None;
    let ctx = ui.ctx().clone();
    if !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Questionmark)) {
        *show_help = !*show_help;
    }
    if *show_help && ctx.input(|i| i.key_pressed(Key::Escape)) {
        *show_help = false;
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::central_panel(ui.style()).inner_margin(0.0))
        .show(ui, |ui| v.world.show(ui));

    egui::Area::new(egui::Id::unique("view_controls"))
        .anchor(Align2::LEFT_TOP, [8.0, 8.0])
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if !v.maps.is_empty() {
                        let current = &v.stats.0;
                        let shown = if v.maps.contains(current) {
                            current.as_str()
                        } else {
                            "Choose a map"
                        };
                        egui::ComboBox::from_id_salt("map")
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                for area in &v.maps {
                                    if ui.selectable_label(area == current, area).clicked()
                                        && area != current
                                    {
                                        chosen = Some(area.clone());
                                    }
                                }
                            })
                            .response
                            .on_hover_text("Switch to another map of this site");
                    }
                    if ui.button("Fit map").clicked() {
                        v.world.fit();
                    }
                    if ui.button("Shortcuts (?)").clicked() {
                        *show_help = !*show_help;
                    }
                });
            });
        });

    if let Some(note) = &v.note {
        let mut close = false;
        egui::Area::new(egui::Id::unique("note"))
            .anchor(Align2::CENTER_TOP, [0.0, 48.0])
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(note);
                        close = ui.button("OK").clicked();
                    });
                });
            });
        if close {
            v.note = None;
        }
    }

    egui::Area::new(egui::Id::unique("attribution"))
        .anchor(Align2::RIGHT_BOTTOM, [-4.0, -4.0])
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    match v.attribution_url {
                        Some(url) => ui.hyperlink_to(&v.attribution, url),
                        None => ui.label(&v.attribution),
                    };
                    if let Some(date) = &v.source_date {
                        ui.label(format!("· data of {date}"));
                    }
                });
            });
        });

    egui::Window::new("Keyboard shortcuts")
        .open(show_help)
        .collapsible(false)
        .resizable(false)
        .show(&ctx, |ui| {
            egui::Grid::new("shortcuts").striped(true).show(ui, |ui| {
                for (keys, action) in world::SHORTCUTS {
                    ui.label(*keys);
                    ui.label(*action);
                    ui.end_row();
                }
            });
            // Also reachable without a keyboard, e.g. on touch screens.
            let mut shown = debug.shown;
            if ui.checkbox(&mut shown, "Show frame times (F3)").changed() {
                debug.toggle();
            }
        });
    chosen
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    const USAGE: &str = "Usage: rusthour-viz [--map <file.rhnet>]\n\n\
                         Without --map, shows a demo network.";
    START.get_or_init(std::time::Instant::now);
    let mut args = std::env::args_os().skip(1);
    let mut path = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--map") if path.is_none() => path = args.next().map(std::path::PathBuf::from),
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => {
                eprintln!("unexpected argument {}\n\n{USAGE}", arg.display());
                std::process::exit(2);
            }
        }
    }
    let load = move || {
        let loaded = match &path {
            Some(path) => std::fs::read(path).map_or_else(
                |e| Loaded::Failed(format!("Could not read {}: {e}.", path.display())),
                Loaded::Map,
            ),
            None => Loaded::Demo(None),
        };
        let state = State::from_loaded(loaded);
        if let State::Failed(message) = &state {
            eprintln!("{message}");
        }
        state
    };
    let state = load();
    eframe::run_native(
        "Rust Hour",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(App::new(state, Box::new(load))))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    // First, so panics anywhere below reach the page (eframe's hook chains to it).
    web::install_panic_hook();
    let load = || {
        let progress = Arc::new(Mutex::new(Progress::default()));
        wasm_bindgen_futures::spawn_local(web::load(Arc::clone(&progress)));
        State::Loading(progress)
    };
    // Before the GPU starts up, so both happen at once.
    let state = load();
    wasm_bindgen_futures::spawn_local(async move {
        let canvas = eframe::web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("rusthour_canvas"))
            .and_then(|e| e.dyn_into::<eframe::web_sys::HtmlCanvasElement>().ok())
            .expect("index.html has the canvas");
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                // `move`: `load` is `Copy`, so without it the closure would only
                // borrow it, and the web runner needs `'static`.
                Box::new(move |_cc| Ok(Box::new(App::new(state, Box::new(load))))),
            )
            .await;
        match started {
            Ok(()) => web::hide_status(),
            // Mostly: neither WebGPU nor WebGL2 is available.
            Err(e) => web::show_status(&format!(
                "Rust Hour could not start. Most likely this browser offers neither WebGPU \
                 nor WebGL2: try a current Firefox, Chrome, Edge or Safari with hardware \
                 acceleration on.\n\nDetails: {e:?}"
            )),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusthour_api::{Network, Node, Point, Road};
    use rusthour_import::rhnet::{self, Metadata, OSM_ATTRIBUTION};

    /// A two-node map with a German street name, as the importer writes it.
    fn rhnet_bytes() -> Vec<u8> {
        let p = |x| Point { x, y: 0.0 };
        let network = Network {
            nodes: vec![
                Node {
                    position: p(0.0),
                    osm_node: None,
                },
                Node {
                    position: p(200.0),
                    osm_node: None,
                },
            ],
            roads: vec![Road {
                from_node: rusthour_api::NodeId(0),
                to_node: rusthour_api::NodeId(1),
                geometry: vec![p(0.0), p(200.0)],
                speed_limit_mps: 13.9,
                lanes: 1,
                name: Some("Schulgäßchen".into()),
                osm: None,
                roundabout: false,
            }],
            connections: vec![],
        };
        let metadata = Metadata {
            area: "test".into(),
            source_date: "2026-10-01".into(),
            attribution: OSM_ATTRIBUTION.into(),
            ..Metadata::default()
        };
        rhnet::encode(&metadata, &network)
    }

    #[test]
    fn rhnet_map_loads_with_osm_attribution() {
        let State::Ready(v) = State::from_loaded(Loaded::Map(rhnet_bytes())) else {
            panic!("not ready");
        };
        assert_eq!(v.attribution, "© OpenStreetMap contributors");
        assert_eq!(v.attribution_url, Some(map::OSM_COPYRIGHT_URL));
        assert_eq!(v.source_date.as_deref(), Some("2026-10-01"));
        assert!(v.note.is_none());
        let names: Vec<&str> = v.world.labels.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(names, ["Schulgäßchen"]);
    }

    #[test]
    fn loaded_map_offers_the_other_maps() {
        let mut p = Progress {
            maps: vec!["bensheim-1km".into(), "bensheim-5km".into()],
            result: Some(Loaded::Map(rhnet_bytes())),
            ..Progress::default()
        };
        let State::Ready(v) = State::finish(&mut p) else {
            panic!("not ready");
        };
        assert_eq!(v.maps, ["bensheim-1km", "bensheim-5km"]);
        // The demo shown for an unknown `?map=` still offers the list.
        p.result = Some(Loaded::Demo(Some("no such map".into())));
        p.maps = vec!["bensheim-1km".into()];
        let State::Ready(v) = State::finish(&mut p) else {
            panic!("not ready");
        };
        assert_eq!(v.maps, ["bensheim-1km"]);
    }

    #[test]
    fn corrupt_map_fails_with_a_message() {
        let mut bytes = rhnet_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        let State::Failed(message) = State::from_loaded(Loaded::Map(bytes)) else {
            panic!("a corrupt map must fail");
        };
        assert!(message.contains("corrupt"), "{message}");
    }
}
