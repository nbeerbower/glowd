use std::io;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::protocol::{self, Device, EFFECTS};

const INDEX_HTML: &str = include_str!("static/index.html");
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);

type Resp = Response<io::Cursor<Vec<u8>>>;

struct App {
    devices: Mutex<Vec<Device>>,
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

pub fn run(port: u16) {
    let server = Server::http(("0.0.0.0", port)).unwrap_or_else(|e| {
        eprintln!("failed to bind port {port}: {e}");
        std::process::exit(1);
    });
    println!("glowd listening on http://0.0.0.0:{port}");

    let app = App {
        devices: Mutex::new(Vec::new()),
    };
    match protocol::discover(DISCOVERY_TIMEOUT) {
        Ok(found) => {
            println!("discovered {} device(s)", found.len());
            *app.devices.lock().unwrap() = found;
        }
        Err(e) => eprintln!("initial discovery failed: {e}"),
    }

    for request in server.incoming_requests() {
        if let Err(e) = handle(&app, request) {
            eprintln!("request error: {e}");
        }
    }
}

fn handle(app: &App, mut request: Request) -> io::Result<()> {
    let method = request.method().clone();
    let url = request.url().to_string();

    let response = match (method, url.as_str()) {
        (Method::Get, "/") => Response::from_string(INDEX_HTML).with_header(header("text/html; charset=utf-8")),
        (Method::Get, "/api/devices") => list_devices(app),
        (Method::Get, "/api/effects") => {
            let names: Vec<&str> = EFFECTS.iter().map(|(name, _)| *name).collect();
            ok_json(json!(names))
        }
        (Method::Post, "/api/discover") => rediscover(app),
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
    let with_state: Vec<serde_json::Value> = devices
        .iter()
        .map(|d| {
            let state = protocol::state(&d.ip).ok(); // null if unreachable
            json!({ "ip": d.ip, "mac": d.mac, "model": d.model, "state": state })
        })
        .collect();
    ok_json(json!(with_state))
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
