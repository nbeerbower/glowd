use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Datelike;
use serde::Deserialize;
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::protocol::{self, Device, EFFECTS};
use crate::schedule::{self, Schedule};
use crate::store;

const INDEX_HTML: &str = include_str!("static/index.html");
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
const SCHEDULER_TICK: Duration = Duration::from_secs(20);
const COLORS_FILE: &str = "colors.json";
const NAMES_FILE: &str = "names.json";
const SCHEDULES_FILE: &str = "schedules.json";

type Resp = Response<io::Cursor<Vec<u8>>>;

struct App {
    devices: Mutex<Vec<Device>>,
    palette: Mutex<Vec<String>>,
    /// MAC -> friendly name. Keyed by MAC because IPs can change on re-lease.
    names: Mutex<HashMap<String, String>>,
    schedules: Mutex<Vec<Schedule>>,
    state_dir: PathBuf,
}

#[derive(Deserialize)]
struct PowerReq {
    ip: String,
    on: bool,
}

#[derive(Deserialize)]
struct ColorReq {
    ip: String,
    r: u8,
    g: u8,
    b: u8,
}

#[derive(Deserialize)]
struct EffectReq {
    ip: String,
    name: String,
    #[serde(default = "default_speed")]
    speed: u8,
}

fn default_speed() -> u8 {
    50
}

#[derive(Deserialize)]
struct SavedColorReq {
    hex: String,
}

#[derive(Deserialize)]
struct NameReq {
    mac: String,
    name: String,
}

#[derive(Deserialize)]
struct NewScheduleReq {
    time: String,
    #[serde(default)]
    days: Vec<u8>,
    action: String,
    hex: Option<String>,
    mac: Option<String>,
}

#[derive(Deserialize)]
struct ScheduleIdReq {
    id: u64,
}

/// Accepts "#ff8800" or "ff8800" (any case); returns canonical "#ff8800".
fn normalize_hex(input: &str) -> Option<String> {
    let hex = input.trim().trim_start_matches('#').to_lowercase();
    (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| format!("#{hex}"))
}

/// "#ff8800" -> (255, 136, 0). Expects an already-normalized value.
fn hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

pub fn run(port: u16, state_dir: PathBuf) {
    let server = Server::http(("0.0.0.0", port)).unwrap_or_else(|e| {
        eprintln!("failed to bind port {port}: {e}");
        std::process::exit(1);
    });
    println!("glowd listening on http://0.0.0.0:{port}");

    let palette: Vec<String> = store::load_json(&state_dir, COLORS_FILE);
    let names: HashMap<String, String> = store::load_json(&state_dir, NAMES_FILE);
    let schedules: Vec<Schedule> = store::load_json(&state_dir, SCHEDULES_FILE);
    println!(
        "state dir {} ({} saved colors, {} schedules)",
        state_dir.display(),
        palette.len(),
        schedules.len()
    );
    let app = Arc::new(App {
        devices: Mutex::new(Vec::new()),
        palette: Mutex::new(palette),
        names: Mutex::new(names),
        schedules: Mutex::new(schedules),
        state_dir,
    });
    match protocol::discover(DISCOVERY_TIMEOUT) {
        Ok(found) => {
            println!("discovered {} device(s)", found.len());
            *app.devices.lock().unwrap() = found;
        }
        Err(e) => eprintln!("initial discovery failed: {e}"),
    }

    // The scheduler gets its own reference to the shared state and its own
    // thread; the main thread stays dedicated to serving requests.
    let scheduler_app = Arc::clone(&app);
    std::thread::spawn(move || scheduler_loop(&scheduler_app));

    for request in server.incoming_requests() {
        if let Err(e) = handle(&app, request) {
            eprintln!("request error: {e}");
        }
    }
}

fn scheduler_loop(app: &App) {
    // Remembers the last minute each schedule fired so a schedule runs once
    // per matching minute even though we wake several times inside it.
    let mut fired: HashMap<u64, String> = HashMap::new();
    loop {
        std::thread::sleep(SCHEDULER_TICK);
        let now = chrono::Local::now();
        let hhmm = now.format("%H:%M").to_string();
        let minute_stamp = now.format("%Y-%m-%d %H:%M").to_string();
        let weekday = now.weekday().num_days_from_sunday() as u8;

        let due: Vec<Schedule> = app
            .schedules
            .lock()
            .unwrap()
            .iter()
            .filter(|s| schedule::is_due(s, weekday, &hhmm))
            .filter(|s| fired.get(&s.id) != Some(&minute_stamp))
            .cloned()
            .collect();
        for s in due {
            fired.insert(s.id, minute_stamp.clone());
            run_schedule(app, &s);
        }
    }
}

fn run_schedule(app: &App, s: &Schedule) {
    let devices = app.devices.lock().unwrap().clone();
    let targets = devices
        .iter()
        .filter(|d| s.mac.as_deref().is_none_or(|mac| mac == d.mac));
    for d in targets {
        let result = match s.action.as_str() {
            "on" => protocol::set_power(&d.ip, true),
            "off" => protocol::set_power(&d.ip, false),
            _ => match s.hex.as_deref().and_then(hex_rgb) {
                Some((r, g, b)) => protocol::set_color(&d.ip, r, g, b),
                None => Ok(()),
            },
        };
        match result {
            Ok(()) => println!("schedule {}: {} -> {}", s.id, s.action, d.ip),
            Err(e) => eprintln!("schedule {}: {} failed: {e}", s.id, d.ip),
        }
    }
}

fn handle(app: &App, mut request: Request) -> io::Result<()> {
    let method = request.method().clone();
    let url = request.url().to_string();

    let response = match (method, url.as_str()) {
        (Method::Get, "/") => {
            Response::from_string(INDEX_HTML).with_header(header("text/html; charset=utf-8"))
        }
        (Method::Get, "/api/devices") => list_devices(app),
        (Method::Get, "/api/effects") => {
            let names: Vec<&str> = EFFECTS.iter().map(|(name, _)| *name).collect();
            ok_json(json!(names))
        }
        (Method::Post, "/api/discover") => rediscover(app),
        (Method::Get, "/api/colors") => ok_json(json!(*app.palette.lock().unwrap())),
        (Method::Post, "/api/colors") => edit_palette(app, &mut request, |palette, hex| {
            if !palette.contains(&hex) {
                palette.push(hex);
            }
        }),
        (Method::Post, "/api/colors/remove") => edit_palette(app, &mut request, |palette, hex| {
            palette.retain(|c| *c != hex);
        }),
        (Method::Post, "/api/name") => set_name(app, &mut request),
        (Method::Get, "/api/schedules") => ok_json(json!(*app.schedules.lock().unwrap())),
        (Method::Post, "/api/schedules") => add_schedule(app, &mut request),
        (Method::Post, "/api/schedules/remove") => edit_schedules(app, &mut request, |list, id| {
            list.retain(|s| s.id != id);
        }),
        (Method::Post, "/api/schedules/toggle") => edit_schedules(app, &mut request, |list, id| {
            if let Some(s) = list.iter_mut().find(|s| s.id == id) {
                s.enabled = !s.enabled;
            }
        }),
        (Method::Post, "/api/power") => with_body(&mut request, |req: PowerReq| {
            protocol::set_power(&req.ip, req.on)
        }),
        (Method::Post, "/api/color") => with_body(&mut request, |req: ColorReq| {
            protocol::set_color(&req.ip, req.r, req.g, req.b)
        }),
        (Method::Post, "/api/effect") => with_body(&mut request, |req: EffectReq| {
            protocol::set_effect(&req.ip, &req.name, req.speed)
        }),
        _ => error_json(404, "not found"),
    };
    request.respond(response)
}

fn list_devices(app: &App) -> Resp {
    let devices = app.devices.lock().unwrap().clone();
    let names = app.names.lock().unwrap().clone();
    let with_state: Vec<serde_json::Value> = devices
        .iter()
        .map(|d| {
            let state = protocol::state(&d.ip).ok(); // null if unreachable
            let name = names
                .get(&d.mac)
                .cloned()
                .unwrap_or_else(|| d.model.clone());
            json!({ "ip": d.ip, "mac": d.mac, "model": d.model, "name": name, "state": state })
        })
        .collect();
    ok_json(json!(with_state))
}

fn set_name(app: &App, request: &mut Request) -> Resp {
    let req: NameReq = match serde_json::from_reader(request.as_reader()) {
        Ok(req) => req,
        Err(e) => return error_json(400, &format!("bad request: {e}")),
    };
    let mut names = app.names.lock().unwrap();
    let name = req.name.trim();
    if name.is_empty() {
        names.remove(&req.mac); // empty name reverts to the model name
    } else {
        names.insert(req.mac, name.to_string());
    }
    if let Err(e) = store::save_json(&app.state_dir, NAMES_FILE, &*names) {
        return error_json(500, &format!("failed to save names: {e}"));
    }
    ok_json(json!({ "ok": true }))
}

fn add_schedule(app: &App, request: &mut Request) -> Resp {
    let req: NewScheduleReq = match serde_json::from_reader(request.as_reader()) {
        Ok(req) => req,
        Err(e) => return error_json(400, &format!("bad request: {e}")),
    };
    let hex = match req.hex.as_deref().map(normalize_hex) {
        Some(None) => return error_json(400, "expected a hex color like #ff8800"),
        Some(normalized) => normalized,
        None => None,
    };
    let mut new = Schedule {
        id: 0,
        time: req.time,
        days: req.days,
        action: req.action,
        hex,
        mac: req.mac.filter(|m| !m.is_empty()),
        enabled: true,
    };
    if let Err(e) = schedule::validate(&new) {
        return error_json(400, &e);
    }
    let mut schedules = app.schedules.lock().unwrap();
    new.id = schedules.iter().map(|s| s.id).max().unwrap_or(0) + 1;
    schedules.push(new);
    if let Err(e) = store::save_json(&app.state_dir, SCHEDULES_FILE, &*schedules) {
        return error_json(500, &format!("failed to save schedules: {e}"));
    }
    ok_json(json!(*schedules))
}

/// Apply a mutation to the schedule list by id, persist, return the new list.
fn edit_schedules(
    app: &App,
    request: &mut Request,
    mutate: impl FnOnce(&mut Vec<Schedule>, u64),
) -> Resp {
    let req: ScheduleIdReq = match serde_json::from_reader(request.as_reader()) {
        Ok(req) => req,
        Err(e) => return error_json(400, &format!("bad request: {e}")),
    };
    let mut schedules = app.schedules.lock().unwrap();
    mutate(&mut schedules, req.id);
    if let Err(e) = store::save_json(&app.state_dir, SCHEDULES_FILE, &*schedules) {
        return error_json(500, &format!("failed to save schedules: {e}"));
    }
    ok_json(json!(*schedules))
}

fn rediscover(app: &App) -> Resp {
    match protocol::discover(DISCOVERY_TIMEOUT) {
        Ok(found) => {
            *app.devices.lock().unwrap() = found;
            list_devices(app)
        }
        Err(e) => error_json(502, &format!("discovery failed: {e}")),
    }
}

/// Apply a mutation to the saved palette, persist it, and return the new list.
fn edit_palette(
    app: &App,
    request: &mut Request,
    mutate: impl FnOnce(&mut Vec<String>, String),
) -> Resp {
    let parsed: Result<SavedColorReq, _> = serde_json::from_reader(request.as_reader());
    let req = match parsed {
        Ok(req) => req,
        Err(e) => return error_json(400, &format!("bad request: {e}")),
    };
    let Some(hex) = normalize_hex(&req.hex) else {
        return error_json(400, "expected a hex color like #ff8800");
    };
    let mut palette = app.palette.lock().unwrap();
    mutate(&mut palette, hex);
    if let Err(e) = store::save_json(&app.state_dir, COLORS_FILE, &*palette) {
        return error_json(500, &format!("failed to save palette: {e}"));
    }
    ok_json(json!(*palette))
}

/// Parse the JSON body, run the device command, and map errors to HTTP codes.
fn with_body<T: serde::de::DeserializeOwned>(
    request: &mut Request,
    action: impl FnOnce(T) -> io::Result<()>,
) -> Resp {
    let parsed: Result<T, _> = serde_json::from_reader(request.as_reader());
    match parsed {
        Ok(req) => match action(req) {
            Ok(()) => ok_json(json!({ "ok": true })),
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => error_json(400, &e.to_string()),
            Err(e) => error_json(502, &format!("device error: {e}")),
        },
        Err(e) => error_json(400, &format!("bad request: {e}")),
    }
}

fn header(content_type: &str) -> Header {
    Header::from_bytes("Content-Type", content_type).expect("static header is valid")
}

fn ok_json(value: serde_json::Value) -> Resp {
    Response::from_string(value.to_string()).with_header(header("application/json"))
}

fn error_json(code: u16, message: &str) -> Resp {
    Response::from_string(json!({ "error": message }).to_string())
        .with_header(header("application/json"))
        .with_status_code(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_hex_input() {
        assert_eq!(normalize_hex("#FF8800").as_deref(), Some("#ff8800"));
        assert_eq!(normalize_hex("ff8800").as_deref(), Some("#ff8800"));
        assert_eq!(normalize_hex(" #ff8800 ").as_deref(), Some("#ff8800"));
        assert_eq!(normalize_hex("ff880"), None);
        assert_eq!(normalize_hex("not-hex"), None);
    }

    #[test]
    fn hex_to_rgb() {
        assert_eq!(hex_rgb("#ff8800"), Some((255, 136, 0)));
        assert_eq!(hex_rgb("#000000"), Some((0, 0, 0)));
        assert_eq!(hex_rgb("ff8800"), None); // must be normalized first
    }
}
