//! Rust Hour interactive viewer, native and in the browser (ADR-0003).

mod camera;
mod demo;
mod world;

use eframe::egui::{self, Align2, Key};
use rusthour_api::{Config, InProcess, Network, Simulation};

use world::World;

/// A network to show, with the attribution its source requires.
pub struct MapSource {
    /// The network, in meters.
    pub network: Network,
    /// Shown at all times, e.g. "© OpenStreetMap contributors" for OSM maps.
    pub attribution: &'static str,
    /// Where the attribution links to, e.g. <https://www.openstreetmap.org/copyright>.
    pub attribution_url: Option<&'static str>,
}

/// The map the viewer shows. The one place to switch to decoded `.rhnet`
/// assets (M1.6), with "© OpenStreetMap contributors" as attribution.
fn load_map() -> MapSource {
    demo::map()
}

struct App {
    world: World,
    attribution: &'static str,
    attribution_url: Option<&'static str>,
    show_help: bool,
}

impl App {
    fn new(map: MapSource) -> Self {
        // shortcut: a bad map panics; real map loading gets the Loading/Failed
        // states of ADR-0003 §5 in M1.6.
        let sim = InProcess::with_network(Config::default(), 0, map.network)
            .expect("the bundled map is valid");
        Self {
            world: World::new(sim.network()),
            attribution: map.attribution,
            attribution_url: map.attribution_url,
            show_help: false,
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Questionmark)) {
            self.show_help = !self.show_help;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(0.0))
            .show(ui, |ui| self.world.show(ui));

        egui::Area::new(egui::Id::unique("view_controls"))
            .anchor(Align2::LEFT_TOP, [8.0, 8.0])
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Fit map").clicked() {
                            self.world.fit();
                        }
                        if ui.button("Shortcuts (?)").clicked() {
                            self.show_help = !self.show_help;
                        }
                    });
                });
            });

        egui::Area::new(egui::Id::unique("attribution"))
            .anchor(Align2::RIGHT_BOTTOM, [-4.0, -4.0])
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| match self.attribution_url {
                    Some(url) => ui.hyperlink_to(self.attribution, url),
                    None => ui.label(self.attribution),
                });
            });

        egui::Window::new("Keyboard shortcuts")
            .open(&mut self.show_help)
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
            });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    eframe::run_native(
        "Rust Hour",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(App::new(load_map())))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    wasm_bindgen_futures::spawn_local(async {
        let document = eframe::web_sys::window()
            .and_then(|w| w.document())
            .expect("running in a browser page");
        let canvas = document
            .get_element_by_id("rusthour_canvas")
            .and_then(|e| e.dyn_into::<eframe::web_sys::HtmlCanvasElement>().ok())
            .expect("index.html has the canvas");
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|_cc| Ok(Box::new(App::new(load_map())))),
            )
            .await;
        if let Some(status) = document.get_element_by_id("status") {
            match started {
                Ok(()) => status.remove(),
                // Mostly: neither WebGPU nor WebGL2 is available.
                Err(e) => status.set_text_content(Some(&format!(
                    "Rust Hour could not start: {e:?}. It needs WebGPU or WebGL2; \
                     try a current Firefox, Chrome, Edge or Safari, \
                     with hardware acceleration enabled."
                ))),
            }
        }
    });
}
