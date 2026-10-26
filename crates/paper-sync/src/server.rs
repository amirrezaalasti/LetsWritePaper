//! The relay: a tiny HTTP server holding one shared log per paper.
//!
//! Devices push the records they changed and long-poll for records other
//! devices changed. The relay stores records in the same SQLite format as a
//! project (`<data>/<project>/history.db`) and never interprets document content.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use paper_core::store::Store;
use paper_core::sync::Record;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server};

const MAX_BODY: u64 = 256 * 1024 * 1024;
const MAX_WAIT_SECS: u64 = 30;
/// A device counts as present this long after its last heartbeat.
pub const PRESENCE_TTL_SECS: u64 = 45;
pub const PULL_BATCH: usize = 500;

#[derive(Serialize, Deserialize)]
pub struct CreateRequest {
    pub title: String,
}

#[derive(Serialize, Deserialize)]
pub struct CreateResponse {
    pub id: String,
    pub token: String,
}

#[derive(Serialize, Deserialize)]
pub struct PushRequest {
    pub client: String,
    pub records: Vec<Record>,
}

#[derive(Serialize, Deserialize)]
pub struct PushResponse {
    pub changed: usize,
}

#[derive(Serialize, Deserialize)]
pub struct PresenceRequest {
    pub client: String,
    /// Display name of the person on that device.
    pub name: String,
}

/// A device currently connected to a shared paper.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Peer {
    pub client: String,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct PresenceResponse {
    pub peers: Vec<Peer>,
}

struct Project {
    store: Mutex<Store>,
    token: String,
    /// Bumped on every accepted push; long-polling pulls wait on it.
    version: Mutex<u64>,
    changed: Condvar,
    /// client -> (name, last heartbeat). In memory only: presence is live state.
    presence: Mutex<HashMap<String, (String, Instant)>>,
}

pub struct Relay {
    data: PathBuf,
    secret: Option<String>,
    projects: Mutex<HashMap<String, Arc<Project>>>,
}

fn json_response(status: u16, body: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_data(body.to_string().into_bytes())
        .with_status_code(status)
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
}

fn error(status: u16, msg: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    json_response(status, json!({ "error": msg }))
}

fn header(req: &Request, name: &str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl Relay {
    /// `secret`, when set, is required to create new shared papers.
    pub fn new(data: &Path, secret: Option<String>) -> Result<Arc<Self>> {
        std::fs::create_dir_all(data)?;
        Ok(Arc::new(Relay { data: data.to_path_buf(), secret, projects: Mutex::new(HashMap::new()) }))
    }

    fn project(&self, id: &str) -> Result<Option<Arc<Project>>> {
        if !valid_id(id) {
            return Ok(None);
        }
        let mut map = self.projects.lock().unwrap();
        if let Some(p) = map.get(id) {
            return Ok(Some(p.clone()));
        }
        let dir = self.data.join(id);
        let Ok(token) = std::fs::read_to_string(dir.join("token")) else { return Ok(None) };
        let store = Store::open(&dir.join("history.db"))?;
        let p = Arc::new(Project { store: Mutex::new(store), token: token.trim().to_string(), version: Mutex::new(0), changed: Condvar::new(), presence: Mutex::new(HashMap::new()) });
        map.insert(id.to_string(), p.clone());
        Ok(Some(p))
    }

    fn create(&self, title: &str) -> Result<CreateResponse> {
        let id = uuid::Uuid::new_v4().simple().to_string()[..20].to_string();
        let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let dir = self.data.join(&id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("token"), &token)?;
        std::fs::write(dir.join("title"), title)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir.join("token"), std::fs::Permissions::from_mode(0o600));
        }
        Store::open(&dir.join("history.db"))?;
        Ok(CreateResponse { id, token })
    }

    /// Serve until the process exits. Each request runs on its own thread so
    /// long-polls do not block pushes.
    pub fn serve(self: &Arc<Self>, addr: &str) -> Result<()> {
        let server = Server::http(addr).map_err(|e| anyhow::anyhow!("cannot listen on {addr}: {e}"))?;
        self.serve_on(server)
    }

    pub fn serve_on(self: &Arc<Self>, server: Server) -> Result<()> {
        for req in server.incoming_requests() {
            let relay = self.clone();
            std::thread::spawn(move || {
                let mut req = req;
                let response = relay.handle(&mut req).unwrap_or_else(|e| error(500, &format!("{e:#}")));
                let _ = req.respond(response);
            });
        }
        Ok(())
    }

    /// Bind and serve on a background thread; returns the bound port.
    pub fn spawn(self: &Arc<Self>, addr: &str) -> Result<u16> {
        let server = Server::http(addr).map_err(|e| anyhow::anyhow!("cannot listen on {addr}: {e}"))?;
        let port = server.server_addr().to_ip().map(|a| a.port()).context("relay has no IP address")?;
        let relay = self.clone();
        std::thread::spawn(move || relay.serve_on(server));
        Ok(port)
    }

    fn handle(&self, req: &mut Request) -> Result<Response<std::io::Cursor<Vec<u8>>>> {
        let url = req.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        let params: HashMap<&str, &str> = query.split('&').filter_map(|kv| kv.split_once('=')).collect();
        match (req.method(), parts.as_slice()) {
            (Method::Get, ["v1", "health"]) => Ok(json_response(200, json!({ "ok": true, "service": "letswritepaper-relay" }))),
            (Method::Post, ["v1", "projects"]) => {
                if let Some(secret) = &self.secret {
                    let given = header(req, "X-Relay-Secret").unwrap_or_default();
                    if !constant_time_eq(&given, secret) {
                        return Ok(error(403, "this relay requires a secret to create shared papers"));
                    }
                }
                let body: CreateRequest = read_json(req)?;
                Ok(json_response(200, serde_json::to_value(self.create(&body.title)?)?))
            }
            (method, ["v1", "projects", id, action]) => {
                let Some(project) = self.project(id)? else { return Ok(error(404, "unknown paper")) };
                let token = header(req, "Authorization").and_then(|v| v.strip_prefix("Bearer ").map(str::to_string)).unwrap_or_default();
                if !constant_time_eq(&token, &project.token) {
                    return Ok(error(401, "invalid share token"));
                }
                match (method, *action) {
                    (Method::Post, "push") => {
                        let body: PushRequest = read_json(req)?;
                        let changed = project.store.lock().unwrap().import_records(body.records, &body.client)?.changed;
                        if changed > 0 {
                            *project.version.lock().unwrap() += 1;
                            project.changed.notify_all();
                        }
                        Ok(json_response(200, serde_json::to_value(PushResponse { changed })?))
                    }
                    (Method::Post, "presence") => {
                        let body: PresenceRequest = read_json(req)?;
                        let mut presence = project.presence.lock().unwrap();
                        let name: String = body.name.trim().chars().take(80).collect();
                        presence.insert(body.client.chars().take(64).collect(), (name, Instant::now()));
                        presence.retain(|_, (_, seen)| seen.elapsed() < Duration::from_secs(PRESENCE_TTL_SECS));
                        let mut peers: Vec<Peer> = presence
                            .iter()
                            .map(|(client, (name, _))| Peer { client: client.clone(), name: name.clone() })
                            .collect();
                        peers.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.client.cmp(&b.client)));
                        Ok(json_response(200, serde_json::to_value(PresenceResponse { peers })?))
                    }
                    (Method::Get, "pull") => {
                        let after: i64 = params.get("after").and_then(|v| v.parse().ok()).unwrap_or(0);
                        let client = params.get("client").map(|s| s.to_string());
                        let wait = params.get("wait").and_then(|v| v.parse().ok()).unwrap_or(0u64).min(MAX_WAIT_SECS);
                        let deadline = Instant::now() + Duration::from_secs(wait);
                        loop {
                            let seen = *project.version.lock().unwrap();
                            let batch = project.store.lock().unwrap().export_changes(after, client.as_deref(), PULL_BATCH)?;
                            let now = Instant::now();
                            if !batch.records.is_empty() || now >= deadline {
                                return Ok(json_response(200, serde_json::to_value(batch)?));
                            }
                            let guard = project.version.lock().unwrap();
                            if *guard == seen {
                                let _ = project.changed.wait_timeout(guard, deadline - now).unwrap();
                            }
                        }
                    }
                    _ => Ok(error(404, "not found")),
                }
            }
            _ => Ok(error(404, "not found")),
        }
    }
}

fn read_json<T: serde::de::DeserializeOwned>(req: &mut Request) -> Result<T> {
    let mut body = Vec::new();
    req.as_reader().take(MAX_BODY).read_to_end(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}
