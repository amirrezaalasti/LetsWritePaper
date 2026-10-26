//! Sync client: share a paper to a relay, join one from a link, and keep a
//! project in sync in the background.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use paper_core::project::Project;
use paper_core::store::Store;
use paper_core::sync::{apply_files, capture_files, ChangeBatch};
use paper_core::now_ms;
use serde::{Deserialize, Serialize};

use crate::server::{CreateRequest, CreateResponse, Peer, PresenceRequest, PresenceResponse, PushRequest, PushResponse};

/// Records that arrived from the relay carry this origin, so they are never pushed back.
pub const RELAY_ORIGIN: &str = "relay";
const REMOTE_FILE: &str = "remote.json";
const PUSH_BATCH: usize = 300;
const PULL_WAIT_SECS: u64 = 25;
const PRESENCE_EVERY_SECS: u64 = 10;

/// Saved in `.paper/remote.json` (never synced or committed: it holds the token).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteConfig {
    /// Relay base URL, e.g. `http://192.168.1.20:8787`.
    pub url: String,
    pub project: String,
    pub token: String,
    /// This device's id; the relay does not send a device its own changes.
    pub client: String,
    /// Set when this machine hosts the relay itself (the app's "share on this network").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosted: Option<HostedRelay>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostedRelay {
    pub port: u16,
    pub data: PathBuf,
    /// Required to create papers on the hosted relay.
    pub secret: String,
}

impl RemoteConfig {
    pub fn link(&self) -> String {
        format!("{}/p/{}#{}", self.url.trim_end_matches('/'), self.project, self.token)
    }

    /// The link to give other devices: for a relay hosted on this machine, its
    /// address on the local network rather than the loopback address we use.
    pub fn public_link(&self) -> String {
        match &self.hosted {
            Some(h) => {
                let ip = lan_ip().map(|i| i.to_string()).unwrap_or_else(|| "127.0.0.1".into());
                let mut public = self.clone();
                public.url = format!("http://{ip}:{}", h.port);
                public.link()
            }
            None => self.link(),
        }
    }

    pub fn load(root: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(root.join(paper_core::project::STATE_DIR).join(REMOTE_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let path = root.join(paper_core::project::STATE_DIR).join(REMOTE_FILE);
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn remove(root: &Path) -> Result<()> {
        let path = root.join(paper_core::project::STATE_DIR).join(REMOTE_FILE);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

/// Parse `http(s)://host[:port]/p/<project>#<token>`.
pub fn parse_link(link: &str) -> Result<(String, String, String)> {
    let link = link.trim();
    let (rest, token) = link.split_once('#').ok_or_else(|| anyhow!("share link is missing its #token part"))?;
    let (url, project) = rest.rsplit_once("/p/").ok_or_else(|| anyhow!("not a LetsWritePaper share link: {link}"))?;
    if !(url.starts_with("http://") || url.starts_with("https://")) || project.is_empty() || token.is_empty() {
        bail!("not a LetsWritePaper share link: {link}");
    }
    Ok((url.to_string(), project.trim_end_matches('/').to_string(), token.to_string()))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(PULL_WAIT_SECS + 30)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn check(mut resp: ureq::http::Response<ureq::Body>) -> Result<ureq::http::Response<ureq::Body>> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let body: serde_json::Value = resp.body_mut().read_json().unwrap_or_default();
    let msg = body.get("error").and_then(|e| e.as_str()).unwrap_or("request failed").to_string();
    Err(anyhow!("relay returned {status}: {msg}"))
}

struct Client {
    cfg: RemoteConfig,
    http: ureq::Agent,
}

impl Client {
    fn new(cfg: RemoteConfig) -> Self {
        Client { cfg, http: agent() }
    }

    fn endpoint(&self, action: &str) -> String {
        format!("{}/v1/projects/{}/{action}", self.cfg.url.trim_end_matches('/'), self.cfg.project)
    }

    /// Push every local change not yet pushed. Returns the number of records sent.
    fn push(&self, store: &Store) -> Result<usize> {
        let mut cursor: i64 = store.meta_get("sync.pushed")?.and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut sent = 0;
        loop {
            if store.max_seq()? <= cursor {
                return Ok(sent);
            }
            let batch = store.export_changes(cursor, Some(RELAY_ORIGIN), PUSH_BATCH)?;
            if !batch.records.is_empty() {
                let body = PushRequest { client: self.cfg.client.clone(), records: batch.records.clone() };
                let resp = self
                    .http
                    .post(&self.endpoint("push"))
                    .header("Authorization", &format!("Bearer {}", self.cfg.token))
                    .send_json(&body)?;
                let _: PushResponse = check(resp)?.body_mut().with_config().limit(1 << 20).read_json()?;
                sent += batch.records.len();
            }
            cursor = batch.seq;
            store.meta_set("sync.pushed", &cursor.to_string())?;
            if !batch.more {
                return Ok(sent);
            }
        }
    }

    /// Pull (long-polling up to `wait` seconds) and apply remote changes.
    /// Returns (records applied, shared files changed).
    fn pull(&self, store: &Store, root: &Path, wait: u64) -> Result<(usize, Vec<String>)> {
        let mut cursor: i64 = store.meta_get("sync.pulled")?.and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut applied = 0;
        let mut files = vec![];
        let mut wait = wait;
        loop {
            let url = format!("{}?after={cursor}&client={}&wait={wait}", self.endpoint("pull"), self.cfg.client);
            let resp = self.http.get(&url).header("Authorization", &format!("Bearer {}", self.cfg.token)).call()?;
            let batch: ChangeBatch = check(resp)?.body_mut().with_config().limit(512 << 20).read_json()?;
            let out = store.import_records(batch.records, RELAY_ORIGIN)?;
            applied += out.changed;
            apply_files(store, root, &out.files)?;
            files.extend(out.files);
            cursor = batch.seq;
            store.meta_set("sync.pulled", &cursor.to_string())?;
            if !batch.more {
                return Ok((applied, files));
            }
            wait = 0;
        }
    }

    /// Tell the relay this device is here; returns everyone connected.
    fn presence(&self, name: &str) -> Result<Vec<Peer>> {
        let body = PresenceRequest { client: self.cfg.client.clone(), name: name.to_string() };
        let resp = self
            .http
            .post(&self.endpoint("presence"))
            .header("Authorization", &format!("Bearer {}", self.cfg.token))
            .send_json(&body)?;
        let out: PresenceResponse = check(resp)?.body_mut().with_config().limit(1 << 20).read_json()?;
        Ok(out.peers)
    }
}

/// Create a shared copy of `project` on the relay at `server` and upload it.
/// Returns the share link other devices use to join.
pub fn share(project: &Project, server: &str, secret: Option<&str>) -> Result<RemoteConfig> {
    let server = server.trim_end_matches('/').to_string();
    let http = agent();
    let mut req = http.post(&format!("{server}/v1/projects"));
    if let Some(s) = secret {
        req = req.header("X-Relay-Secret", s);
    }
    let created: CreateResponse = check(
        req.send_json(&CreateRequest { title: project.manifest.paper.title.clone() })
            .with_context(|| format!("cannot reach relay at {server}"))?,
    )?
    .body_mut()
    .read_json()?;
    let cfg = RemoteConfig {
        url: server,
        project: created.id,
        token: created.token,
        client: uuid::Uuid::new_v4().simple().to_string(),
        hosted: None,
    };
    let store = Store::open(&project.db_path())?;
    store.backfill_changelog()?;
    capture_files(&store, project)?;
    store.meta_set("sync.pushed", "0")?;
    store.meta_set("sync.pulled", "0")?;
    Client::new(cfg.clone()).push(&store)?;
    cfg.save(&project.root)?;
    Ok(cfg)
}

/// Download a shared paper into `dir` (created if needed) and configure it to stay in sync.
pub fn join(link: &str, dir: &Path) -> Result<Project> {
    let (url, project_id, token) = parse_link(link)?;
    if dir.join(paper_core::project::MANIFEST).exists() {
        bail!("{} already contains a paper project", dir.display());
    }
    if let Some(outer) = dir.parent().and_then(|p| Project::discover(p).ok()) {
        bail!(
            "{} is inside the paper project at {}; choose a folder outside it",
            dir.display(),
            outer.root.display()
        );
    }
    let state = dir.join(paper_core::project::STATE_DIR);
    std::fs::create_dir_all(&state)?;
    let cfg = RemoteConfig { url, project: project_id, token, client: uuid::Uuid::new_v4().simple().to_string(), hosted: None };
    let store = Store::open(&state.join("history.db"))?;
    let (applied, _) = Client::new(cfg.clone()).pull(&store, dir, 0)?;
    if !dir.join(paper_core::project::MANIFEST).exists() {
        bail!("the shared paper is empty or still uploading ({applied} records received); try again in a moment");
    }
    // Everything local so far came from the relay.
    store.meta_set("sync.pushed", &store.max_seq()?.to_string())?;
    cfg.save(dir)?;
    drop(store);
    Ok(Project::open(dir)?)
}

/// Live state of a background sync, for status bars.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SyncStatus {
    pub link: Option<String>,
    pub connected: bool,
    pub last_error: Option<String>,
    pub last_push_at: Option<i64>,
    pub last_pull_at: Option<i64>,
    pub pushed: usize,
    pub pulled: usize,
    /// Other people connected to the paper right now (empty on relays
    /// without presence support).
    #[serde(default)]
    pub peers: Vec<Peer>,
}

pub struct SyncHandle {
    stop: Arc<AtomicBool>,
    pub status: Arc<Mutex<SyncStatus>>,
}

impl SyncHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn snapshot(&self) -> SyncStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Drop for SyncHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start syncing the project at `root` in the background, if it has a remote.
/// Pushes run every few hundred milliseconds; pulls long-poll the relay.
pub fn start(root: &Path) -> Result<Option<SyncHandle>> {
    let Some(cfg) = RemoteConfig::load(root) else { return Ok(None) };
    let stop = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(SyncStatus { link: Some(cfg.link()), ..Default::default() }));
    let project = Project::open(root)?;

    let set = |status: &Arc<Mutex<SyncStatus>>, f: &dyn Fn(&mut SyncStatus)| {
        if let Ok(mut s) = status.lock() {
            f(&mut s);
        }
    };

    // Push loop.
    {
        let (stop, status, cfg, project) = (stop.clone(), status.clone(), cfg.clone(), project.clone());
        std::thread::spawn(move || {
            let client = Client::new(cfg);
            let Ok(store) = Store::open(&project.db_path()) else { return };
            let mut backoff = 1;
            while !stop.load(Ordering::SeqCst) {
                let result = capture_files(&store, &project).map_err(anyhow::Error::from).and_then(|_| client.push(&store));
                match result {
                    Ok(n) => {
                        backoff = 1;
                        if n > 0 {
                            set(&status, &|s| {
                                s.pushed += n;
                                s.last_push_at = Some(now_ms());
                                s.connected = true;
                                s.last_error = None;
                            });
                        }
                        std::thread::sleep(Duration::from_millis(400));
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        set(&status, &|s| {
                            s.connected = false;
                            s.last_error = Some(msg.clone());
                        });
                        std::thread::sleep(Duration::from_secs(backoff));
                        backoff = (backoff * 2).min(30);
                    }
                }
            }
        });
    }

    // Pull loop.
    {
        let (stop, status, root, cfg) = (stop.clone(), status.clone(), root.to_path_buf(), cfg.clone());
        std::thread::spawn(move || {
            let client = Client::new(cfg);
            let Ok(store) = Store::open(&project.db_path()) else { return };
            let mut backoff = 1;
            while !stop.load(Ordering::SeqCst) {
                match client.pull(&store, &root, PULL_WAIT_SECS) {
                    Ok((n, _)) => {
                        backoff = 1;
                        set(&status, &|s| {
                            s.connected = true;
                            s.last_error = None;
                            s.last_pull_at = Some(now_ms());
                            s.pulled += n;
                        });
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        set(&status, &|s| {
                            s.connected = false;
                            s.last_error = Some(msg.clone());
                        });
                        std::thread::sleep(Duration::from_secs(backoff));
                        backoff = (backoff * 2).min(30);
                    }
                }
            }
        });
    }
    // Presence heartbeat. Failures only clear the list: sync status comes from push/pull.
    {
        let (stop, status) = (stop.clone(), status.clone());
        let client = Client::new(cfg.clone());
        let name = paper_core::Author::local_user().name;
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let peers = client.presence(&name).unwrap_or_default();
                let others: Vec<Peer> = peers.into_iter().filter(|p| p.client != client.cfg.client).collect();
                set(&status, &|s| s.peers = others.clone());
                for _ in 0..PRESENCE_EVERY_SECS * 2 {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        });
    }
    Ok(Some(SyncHandle { stop, status }))
}

/// Best guess at this machine's address on the local network.
pub fn lan_ip() -> Option<std::net::IpAddr> {
    // Connecting a UDP socket sends nothing; it only selects a route.
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}
