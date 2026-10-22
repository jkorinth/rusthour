//! Browser-only glue: the page's status box, panics, and fetching the map
//! named by `maps/manifest.json` (written by the Pages deploy, M1.7).

use std::sync::{Arc, Mutex};

use eframe::wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    HtmlElement, ReadableStreamDefaultReader, ReadableStreamReadResult, RequestCache, RequestInit,
    Response,
};

use crate::{Loaded, Progress, manifest};

fn status() -> Option<HtmlElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id("status")?
        .dyn_into()
        .ok()
}

/// Shows `message` in the page's status box, over the canvas.
pub fn show_status(message: &str) {
    if let Some(status) = status() {
        status.set_text_content(Some(message));
        let _ = status.style().set_property("display", "flex");
    }
}

/// Hides the status box once the app draws itself.
pub fn hide_status() {
    if let Some(status) = status() {
        let _ = status.style().set_property("display", "none");
    }
}

/// Puts panics in the page, not only in the console.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        show_status(&format!(
            "Rust Hour stopped because of an internal error. Reload the page to try \
             again; if it happens again, please report it with the details below.\n\n{info}"
        ));
        previous(info);
    }));
}

/// Fetches the map to show and hands it to the app through `progress`.
pub async fn load(progress: Arc<Mutex<Progress>>) {
    let loaded = fetch_map(&progress).await;
    progress.lock().expect("not poisoned").result = Some(loaded);
}

async fn fetch_map(progress: &Mutex<Progress>) -> Loaded {
    let demo = |why: &str| Loaded::Demo(Some(format!("{why} Showing a small demo town instead.")));
    let no_maps = "No maps are published on this site yet.";
    // Not content-hashed, unlike the maps: always revalidate, or a stale
    // copy can name a map file that a redeploy has removed.
    let manifest = match fetch("maps/manifest.json", true, |_| ()).await {
        // A missing list, or a dev server's fallback page, means no maps.
        Ok(None) => return demo(no_maps),
        Ok(Some(bytes)) if bytes.starts_with(b"<") => return demo(no_maps),
        Ok(Some(bytes)) => match manifest::parse_manifest(&String::from_utf8_lossy(&bytes)) {
            Ok(m) => m,
            Err(e) => return demo(&e),
        },
        Err(e) => {
            return Loaded::Failed(format!(
                "The list of maps could not be downloaded. Check your connection and \
                 retry.\n\nDetails: maps/manifest.json: {e}."
            ));
        }
    };
    let search = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    let entry = match manifest::choose(&manifest, manifest::requested_area(&search)) {
        Ok(entry) => entry,
        Err(why) => return demo(&why),
    };
    {
        let mut p = progress.lock().expect("not poisoned");
        p.area.clone_from(&entry.area);
        p.total = entry.bytes;
    }
    let url = format!("maps/{}", entry.file);
    let fetched = fetch(&url, false, |n| {
        progress.lock().expect("not poisoned").received = n;
    })
    .await;
    match fetched {
        Ok(Some(bytes)) => Loaded::Map(bytes),
        Ok(None) => Loaded::Failed(format!(
            "The map “{}” is missing on this site. Reload the page; the site may have \
             just been updated.\n\nDetails: {url}: HTTP 404.",
            entry.area
        )),
        Err(e) => Loaded::Failed(format!(
            "The map “{}” could not be downloaded. Check your connection and retry.\n\n\
             Details: {url}: {e}.",
            entry.area
        )),
    }
}

/// GETs `url` (relative to the page), reporting the bytes received so far.
/// `Ok(None)` means 404. `no_cache` revalidates any cached copy first.
async fn fetch(
    url: &str,
    no_cache: bool,
    mut on_progress: impl FnMut(u64),
) -> Result<Option<Vec<u8>>, String> {
    let window = web_sys::window().ok_or("no browser window")?;
    let init = RequestInit::new();
    if no_cache {
        init.set_cache(RequestCache::NoCache);
    }
    let response: Response = JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await
        .map_err(|e| js_error(&e))?
        .dyn_into()
        .map_err(|e| js_error(&e))?;
    if response.status() == 404 {
        return Ok(None);
    }
    if !response.ok() {
        return Err(format!(
            "HTTP {} {}",
            response.status(),
            response.status_text()
        ));
    }
    let Some(body) = response.body() else {
        return Ok(Some(Vec::new()));
    };
    // Without options, getReader() always returns a default reader.
    let reader: ReadableStreamDefaultReader = body.get_reader().unchecked_into();
    let mut bytes = Vec::new();
    loop {
        let chunk: ReadableStreamReadResult = JsFuture::from(reader.read())
            .await
            .map_err(|e| js_error(&e))?
            .unchecked_into();
        if chunk.get_done().unwrap_or(false) {
            return Ok(Some(bytes));
        }
        bytes.extend(js_sys::Uint8Array::new(&chunk.get_value()).to_vec());
        on_progress(bytes.len() as u64);
    }
}

fn js_error(e: &JsValue) -> String {
    e.dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| e.as_string())
        .unwrap_or_else(|| format!("{e:?}"))
}
