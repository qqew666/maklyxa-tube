use std::{
    io,
    net::{Ipv4Addr, SocketAddrV4, TcpListener as StdListener},
    sync::Arc,
    thread,
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, copy_bidirectional},
    net::{TcpListener, TcpStream},
    runtime::Builder,
    sync::{Semaphore, oneshot, watch},
    time::timeout,
};
use tokio_native_tls::TlsConnector;
use tokio_socks::tcp::Socks5Stream;
use url::Url;

#[cfg(windows)]
use crate::tor::{Readiness, Tor};

const HOSTS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "youtube-nocookie.com",
    "googlevideo.com",
    "ytimg.com",
    "google.com",
    "googleapis.com",
    "googleusercontent.com",
    "gstatic.com",
    "ggpht.com",
    "sponsor.ajay.app",
    "github.com",
    "githubusercontent.com",
    "githubassets.com",
];
const HANDSHAKE: Duration = Duration::from_secs(10);

#[derive(Clone, Serialize, Deserialize)]
pub struct Route {
    pub country: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

struct Endpoint {
    route: Route,
    host: String,
    port: u16,
    tls: bool,
}

enum Target {
    #[cfg(windows)]
    Direct,
    Configured(Endpoint),
    #[cfg(windows)]
    Tor(Arc<Tor>),
}

impl Route {
    pub fn valid(&self) -> bool {
        self.endpoint().is_some()
            && match (&self.username, &self.password) {
                (None, None) => true,
                (Some(user), Some(password)) => {
                    !user.is_empty()
                        && user.len() <= 255
                        && password.len() <= 255
                        && !user.contains([':', '\r', '\n'])
                        && !password.contains(['\r', '\n'])
                }
                _ => false,
            }
    }

    fn endpoint(&self) -> Option<(String, u16, bool)> {
        if self.country.len() != 2 || !self.country.bytes().all(|c| c.is_ascii_uppercase()) {
            return None;
        }
        let url = Url::parse(&self.url).ok()?;
        let tls = match url.scheme() {
            "https" => true,
            "socks5" => false,
            _ => return None,
        };
        if !matches!(url.path(), "" | "/")
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let host = match url.host()? {
            url::Host::Ipv6(addr) => addr.to_string(),
            other => other.to_string(),
        };
        let port = url.port_or_known_default().filter(|&port| port != 0)?;
        Some((host, port, tls))
    }
}

pub struct Relay {
    current: watch::Sender<usize>,
    endpoints: Arc<Vec<Target>>,
    port: u16,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Relay {
    #[cfg(windows)]
    pub fn start(routes: &[Route], cache: std::path::PathBuf) -> io::Result<Self> {
        if routes.is_empty() {
            return Self::bind(vec![Target::Direct, Target::Tor(Arc::new(Tor::new(cache)))]);
        }
        Self::bind(Self::configured(routes)?)
    }

    fn configured(routes: &[Route]) -> io::Result<Vec<Target>> {
        routes
            .iter()
            .map(|route| {
                let (host, port, tls) = route
                    .endpoint()
                    .ok_or_else(|| io::Error::other("Некорректный маршрут"))?;
                Ok(Target::Configured(Endpoint {
                    route: route.clone(),
                    host,
                    port,
                    tls,
                }))
            })
            .collect()
    }

    fn bind(endpoints: Vec<Target>) -> io::Result<Self> {
        let endpoints = Arc::new(endpoints);
        let upstream = Arc::clone(&endpoints);
        let listener = StdListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let rt = Builder::new_current_thread().enable_all().build()?;
        let tls = native_tls::TlsConnector::new()
            .map(TlsConnector::from)
            .map_err(io::Error::other)?;
        let (current, active) = watch::channel(0);
        let (stop, shutdown) = oneshot::channel();
        let worker = thread::Builder::new()
            .name("youtube-relay".into())
            .spawn(move || {
                rt.block_on(async move {
                    if let Ok(listener) = TcpListener::from_std(listener) {
                        serve(listener, upstream, active, tls, shutdown).await;
                    }
                });
                rt.shutdown_timeout(Duration::from_millis(500));
            })?;
        Ok(Self {
            current,
            endpoints,
            port,
            stop: Some(stop),
            worker: Some(worker),
        })
    }

    pub fn count(&self) -> usize {
        self.endpoints.len()
    }

    pub fn country(&self) -> &str {
        match &self.endpoints[*self.current.borrow()] {
            #[cfg(windows)]
            Target::Direct => "системное",
            Target::Configured(proxy) => &proxy.route.country,
            #[cfg(windows)]
            Target::Tor(_) => "Tor · Европа",
        }
    }

    #[cfg(windows)]
    pub fn readiness(&self) -> Readiness {
        if let Target::Tor(tor) = &self.endpoints[*self.current.borrow()] {
            tor.ready()
        } else {
            Readiness::Ready
        }
    }

    pub fn advance(&self) {
        let current = *self.current.borrow();
        let next = (current + 1) % self.count();
        #[cfg(windows)]
        if let Target::Tor(tor) = &self.endpoints[next] {
            tor.retry();
        }
        if next != current {
            self.current.send_replace(next);
        }
    }

    pub fn proxy_url(&self) -> String {
        format!("socks5h://127.0.0.1:{}", self.port)
    }

    pub fn browser_args(&self) -> String {
        let hosts = serde_json::to_string(HOSTS).expect("serialize domain list");
        let pac = format!(
            r#"
function FindProxyForURL(url, host) {{
    host = host.toLowerCase();
    if (url.substring(0, 6) !== "https:") return "DIRECT";
    var hosts = {hosts};
    for (var i = 0; i < hosts.length; i++) {{
        if (host === hosts[i] || dnsDomainIs(host, "." + hosts[i])) {{
            return "SOCKS5 127.0.0.1:{}";
        }}
    }}
    return "DIRECT";
}}"#,
            self.port
        );
        // Chromium can fall back to DIRECT if a PAC URL fails to load. The script is inline.
        format!(
            " --disable-quic --proxy-pac-url=data:application/x-ns-proxy-autoconfig;base64,{}",
            STANDARD.encode(pac)
        )
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn serve(
    listener: TcpListener,
    endpoints: Arc<Vec<Target>>,
    active: watch::Receiver<usize>,
    tls: TlsConnector,
    mut stop: oneshot::Receiver<()>,
) {
    let capacity = Arc::new(Semaphore::new(96));
    loop {
        tokio::select! {
            _ = &mut stop => return,
            result = listener.accept() => {
                let Ok((mut client, _)) = result else { return };
                let Ok(permit) = Arc::clone(&capacity).try_acquire_owned() else { continue };
                let mut active = active.clone();
                let endpoints = Arc::clone(&endpoints);
                let tls = tls.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let index = *active.borrow_and_update();
                    tokio::select! {
                        _ = active.changed() => {},
                        _ = relay_connection(&mut client, &endpoints[index], &tls) => {},
                    }
                });
            }
        }
    }
}

fn allowed(host: &str) -> bool {
    HOSTS.iter().any(|domain| {
        host == *domain
            || host
                .strip_suffix(domain)
                .is_some_and(|part| part.ends_with('.'))
    })
}

async fn relay_connection(
    client: &mut TcpStream,
    endpoint: &Target,
    tls: &TlsConnector,
) -> io::Result<()> {
    let mut remote = match timeout(HANDSHAKE, async {
        let host = read_target(client).await?;
        match endpoint {
            #[cfg(windows)]
            Target::Direct => TcpStream::connect((host.as_str(), 443))
                .await
                .map(|stream| Box::new(stream) as Box<dyn Tunnel>),
            #[cfg(windows)]
            Target::Tor(tor) => {
                let proxy = tor
                    .socket()
                    .ok_or_else(|| io::Error::other("Tor is not ready"))?;
                let stream = Socks5Stream::connect(proxy, (host.as_str(), 443))
                    .await
                    .map_err(io::Error::other)?;
                Ok(Box::new(stream.into_inner()) as Box<dyn Tunnel>)
            }
            Target::Configured(proxy) => open_tunnel(proxy, tls, &host).await,
        }
    })
    .await
    {
        Ok(Ok(tunnel)) => tunnel,
        _ => {
            let _ = client.write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            return Ok(());
        }
    };
    client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    copy_bidirectional(client, &mut remote).await?;
    Ok(())
}

async fn read_target(client: &mut TcpStream) -> io::Result<String> {
    let mut greeting = [0; 2];
    client.read_exact(&mut greeting).await?;
    let mut methods = vec![0; usize::from(greeting[1])];
    client.read_exact(&mut methods).await?;
    if greeting[0] != 5 || !methods.contains(&0) {
        client.write_all(&[5, 255]).await?;
        return Err(io::Error::other("unsupported SOCKS negotiation"));
    }
    client.write_all(&[5, 0]).await?;
    let mut request = [0; 5];
    client.read_exact(&mut request).await?;
    // Both WebView2 and yt-dlp send the hostname. The upstream resolves it.
    if request[..4] != [5, 1, 0, 3] || request[4] == 0 {
        return Err(io::Error::other("unsupported SOCKS target"));
    }
    let mut bytes = vec![0; usize::from(request[4])];
    client.read_exact(&mut bytes).await?;
    let port = client.read_u16().await?;
    let host = String::from_utf8(bytes)
        .map_err(io::Error::other)?
        .to_ascii_lowercase();
    if port != 443
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        || !allowed(&host)
    {
        return Err(io::Error::other(
            "destination outside the application route",
        ));
    }
    Ok(host)
}

trait Tunnel: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Tunnel for T {}

async fn open_tunnel(
    endpoint: &Endpoint,
    tls: &TlsConnector,
    host: &str,
) -> io::Result<Box<dyn Tunnel>> {
    let proxy = (endpoint.host.as_str(), endpoint.port);
    let login = endpoint
        .route
        .username
        .as_deref()
        .zip(endpoint.route.password.as_deref());
    if !endpoint.tls {
        let target = (host, 443);
        let stream = if let Some((user, password)) = login {
            Socks5Stream::connect_with_password(proxy, target, user, password).await
        } else {
            Socks5Stream::connect(proxy, target).await
        }
        .map_err(io::Error::other)?;
        return Ok(Box::new(stream.into_inner()));
    }

    let tcp = TcpStream::connect(proxy).await?;
    let mut stream = tls
        .connect(&endpoint.host, tcp)
        .await
        .map_err(io::Error::other)?;
    let mut header = format!("CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n");
    if let Some((user, password)) = login {
        header.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            STANDARD.encode(format!("{user}:{password}"))
        ));
    }
    header.push_str("\r\n");
    stream.write_all(header.as_bytes()).await?;
    let mut response = Vec::new();
    // A buffered reader could swallow the beginning of the tunneled stream.
    while response.len() < 16 * 1024 {
        response.push(stream.read_u8().await?);
        if response.ends_with(b"\r\n\r\n") {
            let mut status = response.split(|&b| b == b' ');
            let protocol = status.next();
            let code = status.next();
            if matches!(protocol, Some(b"HTTP/1.0" | b"HTTP/1.1"))
                && code == Some(b"200".as_slice())
            {
                return Ok(Box::new(stream));
            }
            break;
        }
    }
    Err(io::Error::other("HTTPS proxy refused the tunnel"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_proxy_routes_and_destinations() {
        let mut route = Route {
            country: "DE".into(),
            url: "socks5://127.0.0.1:1080".into(),
            username: None,
            password: None,
        };
        assert!(route.valid());
        route.url = "https://relay.example".into();
        assert!(route.valid());
        route.url = "https://name:secret@relay.example".into();
        assert!(!route.valid());
        route.url = "socks5://relay.example".into();
        assert!(!route.valid());
        assert!(allowed("rr1.googlevideo.com"));
        assert!(!allowed("googlevideo.com.example"));
        assert!(!allowed("fakegooglevideo.com"));
        assert!(!allowed("127.0.0.1"));
    }
    #[tokio::test]
    async fn switches_between_live_socks_routes_and_keeps_dns_remote() {
        let (de, first) = socks_fixture(b'D').await;
        let (nl, second) = socks_fixture(b'N').await;
        let routes = [
            Route {
                country: "DE".into(),
                url: de,
                username: None,
                password: None,
            },
            Route {
                country: "NL".into(),
                url: nl,
                username: None,
                password: None,
            },
        ];
        let relay = Relay::bind(Relay::configured(&routes).unwrap()).unwrap();
        assert!(relay.browser_args().contains("--proxy-pac-url=data:"));
        assert_eq!(
            relay.proxy_url(),
            format!("socks5h://127.0.0.1:{}", relay.port)
        );
        assert_eq!(relay.country(), "DE");
        assert_eq!(through_relay(&relay).await, b'D');
        assert_eq!(first.await.unwrap(), "www.youtube.com");

        relay.advance();
        assert_eq!(relay.country(), "NL");
        assert_eq!(through_relay(&relay).await, b'N');
        assert_eq!(second.await.unwrap(), "www.youtube.com");
    }

    async fn socks_fixture(marker: u8) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut peer, _) = listener.accept().await.unwrap();
            let target = read_target(&mut peer).await.unwrap();
            peer.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0, marker])
                .await
                .unwrap();
            target
        });
        (format!("socks5://{addr}"), task)
    }

    async fn through_relay(relay: &Relay) -> u8 {
        timeout(Duration::from_secs(3), async {
            let mut tunnel =
                Socks5Stream::connect(("127.0.0.1", relay.port), ("www.youtube.com", 443))
                    .await
                    .unwrap();
            tunnel.read_u8().await.unwrap()
        })
        .await
        .unwrap()
    }
}
