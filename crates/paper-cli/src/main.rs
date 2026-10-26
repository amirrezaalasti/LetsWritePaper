use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use clap::{Parser, Subcommand};
use paper_compile::{CompileOptions, Compiler};
use paper_core::history::{render_diffs, VersionRef};
use paper_core::store::CommentFilter;
use paper_core::{format_ms, Author, Format, Ownership, Project, Workspace};

#[derive(Parser)]
#[command(name = "paper", version, about = "Version control, live co-editing and AI collaboration for academic papers")]
struct Cli {
    /// Project directory (defaults to the nearest paper.toml above the current directory).
    #[arg(short = 'C', long, global = true)]
    project: Option<PathBuf>,
    /// Act as this author, e.g. `agent:my-bot` or `Ada Lovelace` (default: git user.name).
    #[arg(long, global = true)]
    author: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a paper project (imports existing .tex/.typ/.bib files).
    Init {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, value_parser = ["latex", "typst"], default_value = "latex")]
        format: String,
        /// Skip writing .mcp.json / Claude Code hook configuration.
        #[arg(long)]
        no_agents: bool,
    },
    /// Overview: documents, checkpoints, open reviews and proposals.
    Status,
    /// Edit sessions, newest first.
    Log {
        path: Option<String>,
        #[arg(short = 'n', long, default_value_t = 30)]
        limit: usize,
    },
    /// Word-level, section-aware diff. Versions: now, last, start, #<id>, or a checkpoint name.
    Diff {
        #[arg(default_value = "last")]
        from: String,
        #[arg(default_value = "now")]
        to: String,
        #[arg(long)]
        path: Option<String>,
    },
    /// Who wrote each line.
    Blame { path: String },
    /// Print a document, optionally at a past version.
    Show {
        path: String,
        #[arg(long)]
        at: Option<String>,
    },
    /// Save a named version (and a git commit if this is a git repository).
    Checkpoint {
        name: String,
        #[arg(short, long, default_value = "")]
        message: String,
        /// Milestone kind: submitted, rebuttal, camera-ready, arxiv, ...
        #[arg(long)]
        milestone: Option<String>,
    },
    /// List checkpoints and milestones.
    Checkpoints,
    /// Restore a document to an earlier version (as a new, attributed edit).
    Restore { path: String, version: String },
    /// Suggested edits from agents and co-authors.
    Proposals {
        #[command(subcommand)]
        action: Option<ProposalCmd>,
    },
    /// Peer review rounds and comments.
    Review {
        #[command(subcommand)]
        action: ReviewCmd,
    },
    /// Writing and review skills.
    Skill {
        #[command(subcommand)]
        action: SkillCmd,
    },
    /// Compile the paper and report diagnostics.
    Compile { main: Option<String> },
    /// Run the MCP server on stdio (for Claude Code, Claude Desktop, Cursor, ...).
    Mcp {
        /// Agent name to attribute edits to (default: the MCP client's name).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Adopt a file's on-disk content as an edit by --author (used by agent hooks).
    Ingest {
        path: Option<String>,
        /// Read a Claude Code PostToolUse hook payload from stdin.
        #[arg(long)]
        from_hook: bool,
    },
    /// Headless sync: mirror files, attribute external edits, follow other processes.
    Watch,
    /// Write agent configuration (.mcp.json, Claude Code hooks, AGENTS.md) into the project.
    Agents,
    /// Run a sync relay that devices share papers through.
    Serve {
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        #[arg(long, default_value_t = paper_sync::DEFAULT_PORT)]
        port: u16,
        /// Where shared papers are stored.
        #[arg(long, default_value = "paper-relay-data")]
        data: PathBuf,
        /// Required to create new shared papers (recommended on public servers). Also read from $PAPER_RELAY_SECRET.
        #[arg(long)]
        secret: Option<String>,
    },
    /// Share this paper through a relay; prints a link other devices can join.
    Share {
        /// Relay URL, e.g. https://relay.example.org (see `paper serve`).
        #[arg(long)]
        server: String,
        #[arg(long)]
        secret: Option<String>,
    },
    /// Download a shared paper from its link and keep it in sync.
    Join { link: String, dir: Option<PathBuf> },
    /// Sync with the relay in the foreground (same as `paper watch` on a shared paper; the app does this automatically).
    Sync,
    /// Show this paper's share link and sync settings.
    Remote {
        /// Stop syncing this copy (keeps all local data).
        #[arg(long)]
        disconnect: bool,
    },
}

#[derive(Subcommand)]
enum ProposalCmd {
    List {
        #[arg(long)]
        status: Option<String>,
    },
    Show { id: i64 },
    Accept { id: i64 },
    Reject { id: i64 },
}

#[derive(Subcommand)]
enum ReviewCmd {
    /// Review rounds.
    List,
    /// Comments, with their current location.
    Comments {
        #[arg(long)]
        round: Option<i64>,
        #[arg(long)]
        status: Option<String>,
    },
    /// Respond to a comment.
    Respond {
        id: i64,
        response: String,
        #[arg(long, default_value = "addressed")]
        status: String,
    },
    /// Markdown response-to-reviewers letter.
    Letter,
}

#[derive(Subcommand)]
enum SkillCmd {
    /// All skills and where they come from.
    List,
    Show { name: String },
    /// Install a skill directory (with SKILL.md) into the project, or for all projects with --user.
    Add {
        path: PathBuf,
        #[arg(long)]
        user: bool,
    },
    /// Apply a skill to sections matching a glob (or the whole paper).
    Bind {
        skill: String,
        #[arg(long)]
        section: Option<String>,
    },
    Unbind {
        skill: String,
        #[arg(long)]
        section: Option<String>,
    },
    /// Which skills apply to a section.
    For { section: String },
    /// Scaffold a new project skill.
    New {
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        #[arg(long, default_value = "writing")]
        kind: String,
        #[arg(long)]
        section: Vec<String>,
    },
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn start_dir(cli: &Cli) -> PathBuf {
    cli.project.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()))
}

fn open(cli: &Cli, ownership: Ownership) -> Result<Workspace> {
    let project = Project::discover(&start_dir(cli))?;
    let mut ws = Workspace::open(project, ownership)?;
    ws.set_session(format!("cli:{}", std::process::id()));
    Ok(ws)
}

fn author(cli: &Cli) -> Author {
    cli.author.as_deref().map(Author::parse).unwrap_or_else(Author::local_user)
}

fn run(cli: Cli) -> Result<()> {
    let me = author(&cli);
    match &cli.cmd {
        Cmd::Init { dir, title, format, no_agents } => {
            let dir = if dir.is_absolute() { dir.clone() } else { start_dir(&cli).join(dir) };
            let title = title.clone().unwrap_or_else(|| {
                dir.canonicalize().ok().and_then(|d| d.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_else(|| "Untitled Paper".into())
            });
            let format = if format == "typst" { Format::Typst } else { Format::Latex };
            let project = Project::init(&dir, &title, format)?;
            let ws = Workspace::open(project, Ownership::Auto)?;
            println!("Initialised \"{title}\" in {}", ws.project.root.display());
            println!("Tracking {} document(s): {}", ws.documents().len(), ws.documents().join(", "));
            if !no_agents {
                write_agent_config(&ws.project)?;
            }
        }
        Cmd::Status => {
            let ws = open(&cli, Ownership::Never)?;
            let m = &ws.project.manifest.paper;
            println!("{} [{}{}]", m.title, if m.format == Format::Latex { "LaTeX" } else { "Typst" }, m.venue.as_ref().map(|v| format!(", {v}")).unwrap_or_default());
            println!("root: {}", ws.project.root.display());
            println!("\nDocuments:");
            for d in ws.documents() {
                let words = ws.text(&d)?.split_whitespace().count();
                println!("  {d:<32} {words:>7} words");
            }
            match ws.checkpoints()?.first() {
                Some(c) => println!("\nLast checkpoint: {} ({}, {})", c.name, c.kind, format_ms(c.created_at)),
                None => println!("\nNo checkpoints yet (paper checkpoint <name>)."),
            }
            let open_c = ws.review_comments(&CommentFilter { status: Some("open".into()), ..Default::default() })?;
            let pending = ws.proposals(Some("pending"))?;
            println!("Open review comments: {}   Pending proposals: {}", open_c.len(), pending.len());
            if let Some(e) = ws.log(None, 1)?.first() {
                println!("Last edit: {} by {} ({})", format_ms(e.ended_at), e.author.name, e.author.kind.as_str());
            }
        }
        Cmd::Log { path, limit } => {
            let ws = open(&cli, Ownership::Never)?;
            for e in ws.log(path.as_deref(), *limit)? {
                println!(
                    "#{:<6} {}  {:<30} {:<22} +{:<5} -{:<5} {}",
                    e.last_id,
                    format_ms(e.ended_at),
                    format!("{} ({})", e.author.name, e.author.kind.as_str()),
                    e.doc,
                    e.added,
                    e.removed,
                    e.messages.join("; ")
                );
            }
        }
        Cmd::Diff { from, to, path } => {
            let ws = open(&cli, Ownership::Never)?;
            let from = if from == "last" && ws.checkpoints()?.is_empty() { "start" } else { from.as_str() };
            print!("{}", render_diffs(&ws.diff(&VersionRef::parse(from), &VersionRef::parse(to), path.as_deref())?));
        }
        Cmd::Blame { path } => {
            let ws = open(&cli, Ownership::Never)?;
            let authors = ws.authors()?;
            for l in ws.blame(path)? {
                let who = l.author.as_ref().map(|id| authors.get(id).map(|a| a.name.clone()).unwrap_or_else(|| id.clone())).unwrap_or_default();
                println!("{:>5} {:<20} {}", l.line, who, l.text);
            }
        }
        Cmd::Show { path, at } => {
            let ws = open(&cli, Ownership::Never)?;
            let upto = match at {
                Some(v) => ws.resolve_version(&VersionRef::parse(v))?,
                None => paper_core::Resolved::Now,
            };
            print!("{}", ws.text_at(path, &upto)?.ok_or_else(|| anyhow!("{path} did not exist at that version"))?);
        }
        Cmd::Checkpoint { name, message, milestone } => {
            let mut ws = open(&cli, Ownership::Auto)?;
            let cp = ws.checkpoint(name, message, milestone.as_deref().unwrap_or("checkpoint"), &me)?;
            println!("Saved checkpoint '{}' at #{}{}", cp.name, cp.update_id, cp.git_commit.map(|c| format!(" (git {})", &c[..10])).unwrap_or_default());
        }
        Cmd::Checkpoints => {
            let ws = open(&cli, Ownership::Never)?;
            for c in ws.checkpoints()? {
                println!("{:<24} {:<13} #{:<6} {}  {}  {}", c.name, c.kind, c.update_id, format_ms(c.created_at), c.author.name, c.message);
            }
        }
        Cmd::Restore { path, version } => {
            let mut ws = open(&cli, Ownership::Auto)?;
            match ws.restore(path, &VersionRef::parse(version), &me)? {
                Some(id) => println!("Restored {path} to {version} (#{id})"),
                None => println!("{path} already matches {version}"),
            }
        }
        Cmd::Proposals { action } => {
            let mut ws = open(&cli, Ownership::Auto)?;
            match action.as_ref().unwrap_or(&ProposalCmd::List { status: Some("pending".into()) }) {
                ProposalCmd::List { status } => {
                    for p in ws.proposals(status.as_deref())? {
                        println!("#{:<4} {:<9} {:<22} {:<20} {}", p.id, p.status, p.author.name, p.doc, p.rationale);
                    }
                }
                ProposalCmd::Show { id } => {
                    let p = ws.store().proposal(*id)?;
                    println!("#{} by {} in {} — {}\n", p.id, p.author.name, p.doc, p.rationale);
                    print!("{}", paper_core::textdiff::render_word_diff(&paper_core::textdiff::word_hunks(&p.old_text, &p.new_text, 2)));
                }
                ProposalCmd::Accept { id } => {
                    let out = ws.accept_proposal(*id, &me)?;
                    println!("Accepted #{id} (line {})", out.line);
                }
                ProposalCmd::Reject { id } => {
                    ws.reject_proposal(*id, &me)?;
                    println!("Rejected #{id}");
                }
            }
        }
        Cmd::Review { action } => {
            let mut ws = open(&cli, Ownership::Never)?;
            match action {
                ReviewCmd::List => {
                    for r in ws.review_rounds()? {
                        let n = ws.review_comments(&CommentFilter { round_id: Some(r.id), ..Default::default() })?.len();
                        println!(
                            "#{:<3} {:<9} {:<28} {:<24} {} comments{}",
                            r.id,
                            r.status,
                            r.title,
                            r.persona.clone().unwrap_or_else(|| r.reviewer.name.clone()),
                            n,
                            r.scores.and_then(|s| s.overall).map(|o| format!(", overall {o}")).unwrap_or_default()
                        );
                    }
                }
                ReviewCmd::Comments { round, status } => {
                    for c in ws.review_comments(&CommentFilter { round_id: *round, status: status.clone(), doc: None })? {
                        let at = c.current_line.map(|l| format!("{}:{l}", c.doc)).unwrap_or_else(|| format!("{} (quote changed)", c.doc));
                        println!("#{:<4} R{} [{} · {}] {:<9} {at}\n      \"{}\"\n      {}", c.id, c.round_id, c.severity, c.category, c.status, c.quote, c.body);
                        if let Some(r) = &c.response {
                            println!("      ↳ {r}");
                        }
                    }
                }
                ReviewCmd::Respond { id, response, status } => {
                    let c = ws.respond_to_comment(*id, response, status, &me)?;
                    println!("#{} → {}", c.id, c.status);
                }
                ReviewCmd::Letter => print!("{}", ws.response_letter()?),
            }
        }
        Cmd::Skill { action } => skill_cmd(&cli, action)?,
        Cmd::Compile { main } => {
            let ws = open(&cli, Ownership::Never)?;
            let main = main.clone().unwrap_or_else(|| ws.project.manifest.paper.main.clone());
            let mut sources = HashMap::new();
            for d in ws.documents() {
                sources.insert(d.clone(), ws.text(&d)?);
            }
            let out = Compiler::new().compile(&ws.project.root, &main, &sources, CompileOptions { svg: false, pdf: true });
            for d in &out.diagnostics {
                println!("{}: {}:{}: {}", d.severity, d.file.as_deref().unwrap_or("?"), d.line.unwrap_or(0), d.message);
            }
            println!("{} with {} in {} ms{}", if out.ok { "Compiled" } else { "Failed" }, out.engine, out.millis, out.pdf_path.map(|p| format!(" → {}", p.display())).unwrap_or_default());
            if !out.ok {
                std::process::exit(2);
            }
        }
        Cmd::Mcp { agent } => paper_mcp::serve_stdio(&start_dir(&cli), agent.as_deref())?,
        Cmd::Ingest { path, from_hook } => {
            if *from_hook {
                let mut input = String::new();
                std::io::stdin().read_to_string(&mut input)?;
                // A hook must never block the agent: report problems, exit 0.
                match paper_core::agents::ingest_from_hook(&input, &me) {
                    Ok(Some(msg)) => eprintln!("{msg}"),
                    Ok(None) => {}
                    Err(e) => eprintln!("paper: could not record edit: {e}"),
                }
                return Ok(());
            }
            let mut ws = open(&cli, Ownership::Never)?;
            let file = path.clone().ok_or_else(|| anyhow!("path required"))?;
            let abs = std::env::current_dir()?.join(&file);
            let abs = abs.canonicalize().unwrap_or(abs);
            let rel = ws.project.relative(&abs).ok_or_else(|| anyhow!("{file} is outside the project"))?;
            match ws.ingest_file(&rel, &me, Some("ingested from disk"))? {
                Some(id) => println!("Recorded {rel} as #{id} by {}", me.name),
                None => println!("{rel} unchanged"),
            }
        }
        Cmd::Watch => watch(&cli)?,
        Cmd::Serve { bind, port, data, secret } => {
            let secret = secret.clone().or_else(|| std::env::var("PAPER_RELAY_SECRET").ok()).filter(|s| !s.is_empty());
            let relay = paper_sync::Relay::new(data, secret.clone())?;
            println!("LetsWritePaper relay on http://{bind}:{port} (data: {})", data.display());
            if let Some(ip) = paper_sync::lan_ip() {
                println!("On this network: http://{ip}:{port}");
            }
            if secret.is_none() {
                println!("Note: anyone who can reach this relay can create shared papers; pass --secret on public servers.");
            }
            relay.serve(&format!("{bind}:{port}"))?;
        }
        Cmd::Share { server, secret } => {
            let project = Project::discover(&start_dir(&cli))?;
            let secret = secret.clone().or_else(|| std::env::var("PAPER_RELAY_SECRET").ok());
            let cfg = paper_sync::share(&project, server, secret.as_deref())?;
            println!("Shared. Join from another device with:\n\n  paper join '{}'\n", cfg.link());
            println!("Anyone with this link can read and edit the paper. Keep `paper watch` or the app running to stay in sync.");
        }
        Cmd::Join { link, dir } => {
            let (_, id, _) = paper_sync::parse_link(link)?;
            let dir = dir.clone().unwrap_or_else(|| start_dir(&cli).join(format!("paper-{}", &id[..8.min(id.len())])));
            let project = paper_sync::join(link, &dir)?;
            let ws = Workspace::open(project, Ownership::Auto)?;
            println!("Joined \"{}\" in {} ({} documents).", ws.project.manifest.paper.title, ws.project.root.display(), ws.documents().len());
            println!("Run `paper watch` there (or open it in the app) to keep syncing.");
        }
        // Syncing needs a process that also mirrors files to disk: that is `watch`.
        Cmd::Sync => {
            let project = Project::discover(&start_dir(&cli))?;
            if paper_sync::RemoteConfig::load(&project.root).is_none() {
                bail!("this paper is not shared (paper share --server URL)");
            }
            watch(&cli)?
        }
        Cmd::Remote { disconnect } => {
            let project = Project::discover(&start_dir(&cli))?;
            match paper_sync::RemoteConfig::load(&project.root) {
                None => println!("Not shared. Use `paper share --server <relay-url>`."),
                Some(_) if *disconnect => {
                    paper_sync::RemoteConfig::remove(&project.root)?;
                    println!("Disconnected; this copy keeps its full history.");
                }
                Some(cfg) => {
                    println!("relay:  {}", cfg.url);
                    println!("link:   {}", cfg.link());
                    println!("device: {}", cfg.client);
                }
            }
        }
        Cmd::Agents => {
            let ws = open(&cli, Ownership::Never)?;
            write_agent_config(&ws.project)?;
        }
    }
    Ok(())
}

fn skill_cmd(cli: &Cli, action: &SkillCmd) -> Result<()> {
    let mut ws = open(cli, Ownership::Never)?;
    match action {
        SkillCmd::List => {
            let cfg = ws.project.manifest.skills.clone();
            for s in ws.skills.skills.values() {
                let mut tags = vec![];
                if cfg.global.contains(&s.name) {
                    tags.push("global".to_string());
                }
                for (pat, names) in &cfg.sections {
                    if names.contains(&s.name) {
                        tags.push(format!("section:{pat}"));
                    }
                }
                if !s.sections.is_empty() && !cfg.disabled.contains(&s.name) {
                    tags.push(format!("auto:{}", s.sections.join("|")));
                }
                println!("{:<28} {:<8} {:<8} {}", s.name, s.kind, format!("{:?}", s.source).to_lowercase(), tags.join(" "));
            }
        }
        SkillCmd::Show { name } => {
            let s = ws.skills.get(name).ok_or_else(|| anyhow!("unknown skill {name}"))?;
            println!("# {} ({}, {:?})\n{}\n\n{}", s.name, s.kind, s.source, s.description, s.body);
        }
        SkillCmd::Add { path, user } => {
            let name = paper_core::skills::install_skill(path, if *user { None } else { Some(&ws.project) })?;
            println!("Installed skill '{name}'{}", if *user { " for all projects" } else { "" });
        }
        SkillCmd::Bind { skill, section } => {
            if ws.skills.get(skill).is_none() {
                bail!("unknown skill {skill}");
            }
            ws.bind_skill(section.as_deref(), skill)?;
            println!("Bound {skill} to {}", section.as_deref().unwrap_or("the whole paper"));
        }
        SkillCmd::Unbind { skill, section } => {
            ws.unbind_skill(section.as_deref(), skill)?;
            println!("Unbound {skill}");
        }
        SkillCmd::For { section } => {
            for a in ws.skills_for(Some(section)) {
                println!("{:<28} {}", a.name, a.reason);
            }
        }
        SkillCmd::New { name, description, kind, section } => {
            let body = format!("# {name}\n\nDescribe what an agent should do, step by step, when this skill applies.\n");
            let path = paper_core::skills::create_skill(&ws.project, name, description, kind, section, &body)?;
            println!("Created {} — edit it, then `paper skill bind {name} --section \"<pattern>\"`", path.display());
        }
    }
    Ok(())
}

/// Headless owner: mirrors files, adopts external edits, follows other processes.
fn watch(cli: &Cli) -> Result<()> {
    use notify::{RecursiveMode, Watcher};
    let mut ws = open(cli, Ownership::Auto)?;
    if !ws.is_owner() {
        bail!("another LetsWritePaper process (the app or `paper watch`) already owns this project");
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            for p in ev.paths {
                let _ = tx.send(p);
            }
        }
    })?;
    watcher.watch(&ws.project.root, RecursiveMode::Recursive)?;
    let _sync = paper_sync::start(&ws.project.root)?;
    println!(
        "Watching {}{} (Ctrl-C to stop)",
        ws.project.root.display(),
        if _sync.is_some() { ", syncing with the relay" } else { "" }
    );
    let mut pending: HashMap<String, std::time::Instant> = HashMap::new();
    loop {
        while let Ok(p) = rx.recv_timeout(Duration::from_millis(250)) {
            if let Some(rel) = ws.project.relative(&p) {
                if ws.project.is_tracked_path(&rel) {
                    pending.insert(rel, std::time::Instant::now());
                }
            }
        }
        if ws.poll()? {
            for e in ws.drain_events() {
                if let paper_core::WorkspaceEvent::DocUpdate { path, author, .. } = e {
                    println!("{} ← {} ({})", path, author.name, author.kind.as_str());
                }
            }
        }
        // Debounce so agent hooks (`paper ingest --from-hook`) can claim edits first.
        let ready: Vec<String> = pending.iter().filter(|(_, t)| t.elapsed() > Duration::from_millis(700)).map(|(p, _)| p.clone()).collect();
        for rel in ready {
            pending.remove(&rel);
            if ws.check_disk(&rel)? {
                println!("{rel} ← external edit recorded");
            }
            ws.drain_events();
        }
    }
}

/// Configure Claude Code (and other MCP agents) for this project.
fn write_agent_config(project: &Project) -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let written = paper_core::agents::write_agent_config(project, &paper_core::agents::Launcher { exe: &exe })?;
    println!("Agent setup: {}", written.join(", "));
    Ok(())
}
