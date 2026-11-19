use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    net::{SocketAddr, TcpStream},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use flate2::read::GzDecoder;
use serde::Deserialize;
use tempfile::TempDir;

use crate::tools::{Asset, ToolResult, fetch, https_agent};

const BUNDLE: Asset = Asset {
    url: "https://raw.githubusercontent.com/qqew666/maklyxa-tube/35c808eb004f3b78dc166c09ddd5c844bad3886f/upstream/tor/tor-expert-bundle-windows-x86_64-16.0.1.tar.gz",
    sha256: "9eed3c3ff1fe43df034ce360196c9ec95472560d77ff55614d0cd5120edea27b",
    max_bytes: 32 * 1024 * 1024,
};
const UPSTREAM: Asset = Asset {
    url: "https://dist.torproject.org/torbrowser/16.0.1/tor-expert-bundle-windows-x86_64-16.0.1.tar.gz",
    ..BUNDLE
};
const BOOTSTRAP_LIMIT: Duration = Duration::from_secs(110);
const POLL: Duration = Duration::from_millis(350);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Pending,
    Ready,
    Failed,
}

#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Starting,
    Up(SocketAddr),
    Failed,
}

struct State {
    phase: Phase,
    child: Option<Child>,
    session: Option<TempDir>,
    next: Bridge,
}

pub struct Tor {
    directory: PathBuf,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
}

impl Tor {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            state: Arc::new(Mutex::new(State {
                phase: Phase::Idle,
                child: None,
                session: None,
                next: Bridge::Snowflake,
            })),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn ready(&self) -> Readiness {
        let mut state = self.state.lock().expect("Tor state");
        let phase = state.phase;
        match phase {
            Phase::Idle => {
                state.phase = Phase::Starting;
                let first_bridge = state.next;
                state.next = first_bridge.other();
                let directory = self.directory.clone();
                let shared = Arc::clone(&self.state);
                let stop = Arc::clone(&self.stop);
                let started =
                    thread::Builder::new()
                        .name("tor-bootstrap".into())
                        .spawn(move || {
                            let result =
                                install_and_start(&directory, first_bridge, &shared, &stop);
                            let mut state = shared.lock().expect("Tor state");
                            if let Ok(socket) = result
                                && !stop.load(Ordering::Acquire)
                            {
                                state.phase = Phase::Up(socket);
                            } else {
                                state.terminate();
                                state.phase = Phase::Failed;
                            }
                        });
                if started.is_err() {
                    state.phase = Phase::Failed;
                    return Readiness::Failed;
                }
                Readiness::Pending
            }
            Phase::Starting => Readiness::Pending,
            Phase::Up(_) if state.running() => Readiness::Ready,
            Phase::Up(_) | Phase::Failed => {
                state.terminate();
                state.phase = Phase::Failed;
                Readiness::Failed
            }
        }
    }

    pub fn retry(&self) {
        let mut state = self.state.lock().expect("Tor state");
        if matches!(state.phase, Phase::Failed) {
            state.phase = Phase::Idle;
        }
    }

    pub fn socket(&self) -> Option<SocketAddr> {
        let state = self.state.lock().expect("Tor state");
        match state.phase {
            Phase::Up(socket) => Some(socket),
            _ => None,
        }
    }
}

impl Drop for Tor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.state.lock().expect("Tor state").terminate();
    }
}

impl State {
    fn running(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }

    fn terminate(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.session.take();
    }
}

#[derive(Clone, Copy)]
enum Bridge {
    Snowflake,
    Obfs4,
}

impl Bridge {
    fn other(self) -> Self {
        match self {
            Self::Snowflake => Self::Obfs4,
            Self::Obfs4 => Self::Snowflake,
        }
    }

    fn keyword(self) -> &'static str {
        match self {
            Self::Snowflake => "snowflake",
            Self::Obfs4 => "obfs4",
        }
    }
}

#[derive(Deserialize)]
struct UpstreamConfig {
    bridges: std::collections::HashMap<String, Vec<String>>,
}

fn install_and_start(
    directory: &Path,
    first: Bridge,
    state: &Mutex<State>,
    stop: &AtomicBool,
) -> ToolResult<SocketAddr> {
    let bundle = ensure_bundle(directory)?;
    for bridge in [first, first.other()] {
        check_stop(stop)?;
        if let Ok(socket) = start_attempt(directory, &bundle, bridge, state, stop) {
            return Ok(socket);
        }
        state.lock().expect("Tor state").terminate();
    }
    Err(io::Error::other("Tor could not connect using the built-in bridges").into())
}

fn ensure_bundle(directory: &Path) -> ToolResult<PathBuf> {
    let bundle = directory.join("expert-16.0.1");
    if bundle.join("tor/tor.exe").is_file() {
        return Ok(bundle);
    }
    fs::create_dir_all(directory)?;
    let agent = https_agent(None, Duration::from_secs(75))?;
    let archive =
        fetch(&agent, &BUNDLE, directory).or_else(|_| fetch(&agent, &UPSTREAM, directory))?;
    let staging = TempDir::new_in(directory)?;
    tar::Archive::new(GzDecoder::new(archive.reopen()?)).unpack(staging.path())?;
    if !staging.path().join("tor/tor.exe").is_file() {
        return Err(io::Error::other("invalid Tor archive layout").into());
    }
    fs::rename(staging.path(), &bundle)?;
    Ok(bundle)
}

fn start_attempt(
    directory: &Path,
    bundle: &Path,
    bridge: Bridge,
    state: &Mutex<State>,
    stop: &AtomicBool,
) -> ToolResult<SocketAddr> {
    let session = TempDir::new_in(directory)?;
    let config = session.path().join("torrc");
    let control_file = session.path().join("control-port");
    let cookie = session.path().join("control-cookie");
    let transport_path = bundle.join("tor/pluggable_transports");
    let settings: UpstreamConfig =
        serde_json::from_slice(&fs::read(transport_path.join("pt_config.json"))?)?;
    let data = directory.join("state");
    fs::create_dir_all(&data)?;
    let mut contents = format!(
        "ClientOnly 1\nAvoidDiskWrites 1\nDataDirectory {}\nGeoIPFile {}\nGeoIPv6File {}\nSocksPort 127.0.0.1:auto\nControlPort 127.0.0.1:auto\nControlPortWriteToFile {}\nCookieAuthentication 1\nCookieAuthFile {}\nUseBridges 1\nExitNodes {{de}},{{nl}},{{fi}},{{se}},{{pl}},{{fr}}\nClientTransportPlugin {} exec ./lyrebird.exe\n",
        tor_path(&data),
        tor_path(&bundle.join("data/geoip")),
        tor_path(&bundle.join("data/geoip6")),
        tor_path(&control_file),
        tor_path(&cookie),
        bridge.keyword()
    );
    let bridges = settings
        .bridges
        .get(bridge.keyword())
        .filter(|list| !list.is_empty())
        .ok_or_else(|| {
            io::Error::other("Tor bundle has no built-in bridges for the selected transport")
        })?;
    for bridge in bridges {
        contents.push_str("Bridge ");
        contents.push_str(bridge);
        contents.push('\n');
    }
    fs::write(&config, contents)?;
    {
        let mut state = state.lock().expect("Tor state");
        check_stop(stop)?;
        let child = Command::new(bundle.join("tor/tor.exe"))
            .current_dir(&transport_path)
            .arg("-f")
            .arg(config)
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        state.child = Some(child);
        state.session = Some(session);
    }
    let until = Instant::now() + BOOTSTRAP_LIMIT;
    while Instant::now() < until {
        check_stop(stop)?;
        if !state.lock().expect("Tor state").running() {
            return Err(io::Error::other("Tor exited before bootstrapping").into());
        }
        if let Ok(Some(socket)) = bootstrapped(&control_file, &cookie) {
            return Ok(socket);
        }
        thread::sleep(POLL);
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "Tor bootstrap timed out").into())
}

fn check_stop(stop: &AtomicBool) -> io::Result<()> {
    if stop.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "application closed",
        ));
    }
    Ok(())
}

fn tor_path(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "/")
            .replace('"', "\\\"")
    )
}

fn bootstrapped(control_file: &Path, cookie: &Path) -> io::Result<Option<SocketAddr>> {
    let address: SocketAddr = fs::read_to_string(control_file)?
        .trim()
        .strip_prefix("PORT=")
        .and_then(|port| port.parse().ok())
        .filter(|addr: &SocketAddr| addr.ip().is_loopback())
        .ok_or_else(|| io::Error::other("Tor control port unavailable"))?;
    let cookie = fs::read(cookie)?;
    if cookie.len() != 32 {
        return Err(io::Error::other("Tor control cookie unavailable"));
    }
    let deadline = Duration::from_secs(2);
    let tcp = TcpStream::connect_timeout(&address, deadline)?;
    tcp.set_read_timeout(Some(deadline))?;
    tcp.set_write_timeout(Some(deadline))?;
    let mut stream = BufReader::new(tcp);
    let hex: String = cookie.iter().map(|byte| format!("{byte:02x}")).collect();
    query(&mut stream, &format!("AUTHENTICATE {hex}"))?;
    let state = query(&mut stream, "GETINFO status/bootstrap-phase")?;
    if !state
        .split_ascii_whitespace()
        .any(|field| field == "PROGRESS=100")
    {
        return Ok(None);
    }
    let listeners = query(&mut stream, "GETINFO net/listeners/socks")?;
    Ok(listeners.lines().find_map(|line| {
        line.strip_prefix("250-net/listeners/socks=")?
            .split_ascii_whitespace()
            .find_map(|address| {
                address
                    .trim_matches('"')
                    .parse::<SocketAddr>()
                    .ok()
                    .filter(|addr| addr.ip().is_loopback())
            })
    }))
}

fn query(stream: &mut BufReader<TcpStream>, command: &str) -> io::Result<String> {
    write!(stream.get_mut(), "{command}\r\n")?;
    let mut reply = String::new();
    for _ in 0..8 {
        let mut line = String::new();
        if stream.read_line(&mut line)? == 0 || !line.starts_with("250") {
            break;
        }
        reply.push_str(&line);
        if line.starts_with("250 ") {
            return Ok(reply);
        }
    }
    Err(io::Error::other("unexpected Tor control response"))
}
