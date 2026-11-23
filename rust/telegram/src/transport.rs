// Telegram's local proxy routes, matching the order of the pinned Flowseal engine.
use crate::{
    protocol::{Session, Splitter},
    runtime_config::Config,
    transport_h2::H2Pool,
};
use aes::cipher::StreamCipher;
use futures_util::{FutureExt, SinkExt, StreamExt, stream::FuturesUnordered};
use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::{Notify, Semaphore, watch},
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, client_async_with_config,
    tungstenite::{
        Error as WsError, Message,
        client::IntoClientRequest,
        error::{ProtocolError, SubProtocolError},
        protocol::WebSocketConfig,
    },
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const LIMIT: usize = 16 * 1024 * 1024;
const DOMAIN_LIST: &str =
    "https://raw.githubusercontent.com/Flowseal/tg-ws-proxy/main/.github/cfproxy-domains.txt";
type Shared<T> = Arc<Mutex<T>>;

fn ioe(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
fn valid_domain(name: &str) -> bool {
    let labels: Vec<_> = name.split('.').collect();
    name.len() <= 253
        && labels.len() > 1
        && labels.iter().all(|s| {
            !s.is_empty()
                && s.len() <= 63
                && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                && !s.starts_with('-')
                && !s.ends_with('-')
        })
        && labels
            .last()
            .is_some_and(|s| s.len() >= 2 && s.bytes().any(|v| v.is_ascii_alphabetic()))
}
fn decode_domain(input: &str) -> String {
    // The upstream list encodes .co.uk hosts as rotated .com labels.
    if let Some(label) = input.strip_suffix(".com") {
        let rotation = (label.bytes().filter(u8::is_ascii_alphabetic).count() % 26) as u8;
        let mut s: String = label
            .bytes()
            .map(|b| {
                if b.is_ascii_alphabetic() {
                    ((b.to_ascii_lowercase() - b'a' + 26 - rotation) % 26 + b'a') as char
                } else {
                    b as char
                }
            })
            .collect();
        s.push_str(".co.uk");
        s
    } else {
        input.to_ascii_lowercase()
    }
}
fn dc_ip(dc: u16, test: bool) -> Option<&'static str> {
    match (test, dc) {
        (false, 1) => Some("149.154.175.50"),
        (false, 2) => Some("149.154.167.51"),
        (false, 3) => Some("149.154.175.100"),
        (false, 4) => Some("149.154.167.91"),
        (false, 5) => Some("149.154.171.5"),
        (false, 203) => Some("91.105.192.100"),
        (true, 1) => Some("149.154.175.10"),
        (true, 2) => Some("149.154.167.40"),
        (true, 3) => Some("149.154.175.117"),
        _ => None,
    }
}
async fn ws_open(
    target: &str,
    domain: &str,
    sni: &str,
    path: &str,
    secure: bool,
    buffer: usize,
) -> io::Result<Ws> {
    let address = (target, if secure { 443 } else { 80 });
    ws_open_at(address, domain, sni, path, secure, buffer).await
}

async fn ws_open_at(
    address: (&str, u16),
    domain: &str,
    sni: &str,
    path: &str,
    secure: bool,
    buffer: usize,
) -> io::Result<Ws> {
    if !valid_domain(domain) || !valid_domain(sni) {
        return Err(ioe("Недопустимое имя сервера."));
    }
    let uri = format!("{}://{domain}{path}", if secure { "wss" } else { "ws" });
    let handshake = async |binary: bool| -> Result<Ws, WsError> {
        let tcp = TcpStream::connect(address).await?;
        tcp.set_nodelay(true)?;
        let sock = socket2::SockRef::from(&tcp);
        let _ = sock.set_send_buffer_size(buffer);
        let _ = sock.set_recv_buffer_size(buffer);
        let stream = if secure {
            let tls = native_tls::TlsConnector::new().map_err(ioe)?;
            MaybeTlsStream::NativeTls(
                tokio_native_tls::TlsConnector::from(tls)
                    .connect(sni, tcp)
                    .await
                    .map_err(ioe)?,
            )
        } else {
            MaybeTlsStream::Plain(tcp)
        };
        let mut request = uri.as_str().into_client_request()?;
        if binary {
            request
                .headers_mut()
                .insert("Sec-WebSocket-Protocol", "binary".parse().unwrap());
        }
        let options = WebSocketConfig::default()
            .max_frame_size(Some(LIMIT))
            .max_message_size(Some(LIMIT));
        client_async_with_config(request, stream, Some(options))
            .await
            .map(|(ws, _)| ws)
    };
    timeout(Duration::from_secs(10), async {
        match handshake(true).await {
            // Some compatible endpoints omit the optional subprotocol. The first
            // connection was consumed by the handshake; negotiate a fresh one.
            Err(WsError::Protocol(ProtocolError::SecWebSocketSubProtocolError(
                SubProtocolError::NoSubProtocol,
            ))) => handshake(false).await,
            result => result,
        }
    })
    .await
    .map_err(ioe)?
    .map_err(ioe)
}

#[derive(Clone, Hash, PartialEq, Eq)]
enum Route {
    Direct(u16, bool, bool),
    Worker(u16),
}
struct Queue {
    idle: VecDeque<(Instant, Ws)>,
    wake: Arc<Notify>,
}
type Buckets = Shared<HashMap<Route, Queue>>;
struct Backoff {
    until: Instant,
    failures: u8,
}
pub struct Pools {
    buckets: Buckets,
    limit: Arc<Semaphore>,
    stop: watch::Sender<()>,
    domains: Shared<Vec<String>>,
    preferred: Mutex<HashMap<u16, String>>,
    backoff: Mutex<HashMap<String, Backoff>>,
    tcp_connecting: Mutex<std::collections::HashSet<String>>,
    h2: H2Pool,
}

async fn direct(route: &Route, config: &Config) -> Option<Ws> {
    match *route {
        Route::Direct(dc, media, test) => {
            let ip = config.dc_redirects.get(&dc)?.to_string();
            let num = if dc == 203 { 2 } else { dc };
            let mut domains = Vec::new();
            if media {
                domains.push(format!("kws{num}-1.web.telegram.org"));
            }
            domains.push(format!("kws{num}.web.telegram.org"));
            for domain in domains {
                for hostname in [&domain[..], "sprinthost.ru"] {
                    if let Ok(ws) = ws_open(
                        &ip,
                        &domain,
                        hostname,
                        if test { "/apiws_test" } else { "/apiws" },
                        true,
                        config.buffer_size,
                    )
                    .await
                    {
                        return Some(ws);
                    }
                }
            }
        }
        Route::Worker(dc) => {
            let path = format!("/apiws?dst={}&dc={dc}", dc_ip(dc, false)?);
            for domain in &config.cfproxy_worker_domains {
                if let Ok(ws) = ws_open(
                    domain,
                    domain,
                    domain,
                    &path,
                    !config.no_secure,
                    config.buffer_size,
                )
                .await
                {
                    return Some(ws);
                }
            }
        }
    }
    None
}
impl Pools {
    pub fn new(config: &Config) -> io::Result<Self> {
        let domains = if config.cfproxy_user_domains.is_empty() {
            include_str!("default-domains.txt")
                .lines()
                .map(str::to_owned)
                .collect()
        } else {
            config.cfproxy_user_domains.clone()
        };
        let (stop, _) = watch::channel(());
        Ok(Self {
            buckets: Default::default(),
            limit: Arc::new(Semaphore::new(32)),
            stop,
            domains: Arc::new(Mutex::new(domains)),
            preferred: Default::default(),
            backoff: Default::default(),
            tcp_connecting: Default::default(),
            h2: H2Pool::new()?,
        })
    }
    fn maintain(&self, route: Route, config: &Config) -> Arc<Notify> {
        let mut buckets = self.buckets.lock().unwrap();
        if let Some(q) = buckets.get(&route) {
            return q.wake.clone();
        }
        let wake = Arc::new(Notify::new());
        buckets.insert(
            route.clone(),
            Queue {
                idle: VecDeque::new(),
                wake: wake.clone(),
            },
        );
        let all = self.buckets.clone();
        let limits = self.limit.clone();
        let config = config.clone();
        let signal = wake.clone();
        let mut stop = self.stop.subscribe();
        tokio::spawn(async move {
            let fill = async {
                let mut retry = Duration::from_secs(1);
                loop {
                    let missing = {
                        let mut map = all.lock().unwrap();
                        let q = map.get_mut(&route).unwrap();
                        q.idle
                            .retain(|(t, _)| t.elapsed() < Duration::from_secs(120));
                        config.pool_size.saturating_sub(q.idle.len())
                    };
                    if missing > 0 {
                        let mut jobs = FuturesUnordered::new();
                        let mut success = false;
                        for _ in 0..missing {
                            jobs.push(async {
                                let _permit = limits.acquire().await.ok()?;
                                direct(&route, &config).await
                            });
                        }
                        while let Some(ws) = jobs.next().await {
                            if let Some(ws) = ws {
                                all.lock()
                                    .unwrap()
                                    .get_mut(&route)
                                    .unwrap()
                                    .idle
                                    .push_back((Instant::now(), ws));
                                success = true;
                            }
                        }
                        if !success {
                            tokio::time::sleep(retry).await;
                            retry = (retry * 2).min(Duration::from_secs(3600));
                            continue;
                        }
                        retry = Duration::from_secs(1);
                    }
                    tokio::select! {_=signal.notified()=>{},_=tokio::time::sleep(Duration::from_secs(5))=>{}}
                }
            };
            tokio::select! {_=stop.changed()=>{},_=fill=>{}}
        });
        wake
    }
    fn take(&self, route: Route, config: &Config) -> Option<Ws> {
        if config.pool_size == 0 {
            return None;
        }
        let signal = self.maintain(route.clone(), config);
        let mut buckets = self.buckets.lock().unwrap();
        let idle = &mut buckets.get_mut(&route).unwrap().idle;
        signal.notify_one();
        while let Some((created, mut ws)) = idle.pop_front() {
            if created.elapsed() < Duration::from_secs(120) && ws.next().now_or_never().is_none() {
                return Some(ws);
            }
        }
        None
    }
    pub async fn warmup(&self, config: &Config) {
        if config.pool_size > 0 {
            for &dc in config.dc_redirects.keys() {
                for media in [false, true] {
                    self.maintain(Route::Direct(dc, media, config.force_test_dc), config);
                }
            }
            if !config.cfproxy_worker_domains.is_empty() && !config.force_test_dc {
                for dc in [1, 2, 3, 4, 5, 203] {
                    self.maintain(Route::Worker(dc), config);
                }
            }
        }
        if config.cfproxy && config.cfproxy_user_domains.is_empty() {
            let domains = self.domains.clone();
            let mut stop = self.stop.subscribe();
            tokio::spawn(async move {
                let fetch = async {
                    let client = match reqwest::Client::builder()
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .timeout(Duration::from_secs(10))
                        .build()
                    {
                        Ok(c) => c,
                        Err(_) => return,
                    };
                    loop {
                        if let Ok(mut r) = client.get(DOMAIN_LIST).send().await {
                            if r.status().is_success() {
                                let mut bytes = Vec::new();
                                while let Ok(Some(chunk)) = r.chunk().await {
                                    if bytes.len() + chunk.len() > 65536 {
                                        bytes.clear();
                                        break;
                                    }
                                    bytes.extend_from_slice(&chunk);
                                }
                                if let Ok(text) = std::str::from_utf8(&bytes) {
                                    let mut update = Vec::new();
                                    for line in text
                                        .lines()
                                        .map(str::trim)
                                        .filter(|s| !s.is_empty() && !s.starts_with('#'))
                                    {
                                        let domain = decode_domain(line);
                                        if valid_domain(&domain) && !update.contains(&domain) {
                                            update.push(domain);
                                        }
                                    }
                                    if update.len() >= 3 {
                                        *domains.lock().unwrap() = update;
                                    }
                                }
                            }
                        }
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                    }
                };
                tokio::select! {_=stop.changed()=>{},_=fetch=>{}}
            });
        }
    }
    fn domains_for(&self, dc: u16) -> Vec<String> {
        let mut domains = self.domains.lock().unwrap().clone();
        if !domains.is_empty() {
            let mut r = [0; 2];
            let _ = getrandom::fill(&mut r);
            let len = domains.len();
            domains.rotate_left(usize::from(u16::from_ne_bytes(r)) % len);
        }
        if let Some(prior) = self.preferred.lock().unwrap().get(&dc) {
            if let Some(i) = domains.iter().position(|x| x == prior) {
                domains.swap(0, i);
            }
        }
        domains
    }
    fn available(&self, s: &str) -> bool {
        self.backoff
            .lock()
            .unwrap()
            .get(s)
            .is_none_or(|b| b.until <= Instant::now())
    }
    fn record(&self, s: &str, ok: bool) {
        let mut map = self.backoff.lock().unwrap();
        if ok {
            map.remove(s);
        } else {
            let failures = map.get(s).map_or(1, |x| x.failures.saturating_add(1));
            map.insert(
                s.into(),
                Backoff {
                    until: Instant::now() + Duration::from_secs(1u64 << failures.min(7)),
                    failures,
                },
            );
        }
    }
}
async fn via_ws<S: AsyncRead + AsyncWrite + Unpin>(
    client: S,
    mut session: Session,
    mut ws: Ws,
) -> io::Result<()> {
    ws.send(Message::Binary(session.init.to_vec().into()))
        .await
        .map_err(ioe)?;
    let (mut up, mut down) = ws.split();
    let (mut input, mut output) = tokio::io::split(client);
    let upload = async {
        let mut frame = vec![0; 65536];
        let mut split = Splitter::new(session.tag);
        loop {
            let n = input.read(&mut frame).await?;
            if n == 0 {
                if let Some(mut buf) = split.flush() {
                    session.relay_out.apply_keystream(&mut buf);
                    up.send(Message::Binary(buf.into())).await.map_err(ioe)?;
                }
                return Ok(());
            }
            session.client_in.apply_keystream(&mut frame[..n]);
            for mut piece in split.split(&frame[..n]) {
                session.relay_out.apply_keystream(&mut piece);
                up.send(Message::Binary(piece.into())).await.map_err(ioe)?;
            }
        }
    };
    let download = async {
        while let Some(frame) = down.next().await {
            match frame.map_err(ioe)? {
                Message::Binary(data) => {
                    let mut buf = data.to_vec();
                    session.relay_in.apply_keystream(&mut buf);
                    session.client_out.apply_keystream(&mut buf);
                    output.write_all(&buf).await?;
                    output.flush().await?;
                }
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) => {}
                _ => return Err(ioe("Неверный кадр Telegram WebSocket.")),
            }
        }
        Ok(())
    };
    tokio::select! {r=upload=>r,r=download=>r}
}
async fn via_tcp<S: AsyncRead + AsyncWrite + Unpin>(
    client: S,
    session: Session,
    mut upstream: TcpStream,
) -> io::Result<()> {
    upstream.write_all(&session.init).await?;
    let (mut input, mut output) = tokio::io::split(client);
    let (mut remote_in, mut remote_out) = upstream.split();
    async fn copy<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        r: &mut R,
        w: &mut W,
        mut decode: crate::protocol::Cipher,
        mut encode: crate::protocol::Cipher,
    ) -> io::Result<()> {
        let mut buf = vec![0; 65536];
        loop {
            let n = r.read(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            decode.apply_keystream(&mut buf[..n]);
            encode.apply_keystream(&mut buf[..n]);
            w.write_all(&buf[..n]).await?;
            w.flush().await?;
        }
    }
    tokio::select! {r=copy(&mut input,&mut remote_out,session.client_in,session.relay_out)=>r,r=copy(&mut remote_in,&mut output,session.relay_in,session.client_out)=>r}
}
pub async fn serve<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    client: S,
    session: Session,
    config: &Config,
    pools: &Pools,
) -> io::Result<()> {
    let (dc, test, media) = (session.dc, session.test, session.media);
    if config.dc_redirects.contains_key(&dc) {
        if let Some(ws) = pools.take(Route::Direct(dc, media, test), config) {
            return via_ws(client, session, ws).await;
        }
    }
    let dest = dc_ip(dc, test);
    if let Some(ip) = dest.filter(|_| !config.cfproxy_worker_domains.is_empty()) {
        if !test {
            if let Some(ws) = pools.take(Route::Worker(dc), config) {
                return via_ws(client, session, ws).await;
            }
        }
        let path = format!("/apiws?dst={ip}&dc={dc}");
        for host in &config.cfproxy_worker_domains {
            if !pools.available(host) {
                continue;
            }
            let ws = ws_open(
                host,
                host,
                host,
                &path,
                !config.no_secure,
                config.buffer_size,
            )
            .await;
            pools.record(host, ws.is_ok());
            if let Ok(ws) = ws {
                return via_ws(client, session, ws).await;
            }
        }
    }
    if !test && config.cfproxy {
        let domains = pools.domains_for(dc);
        if media && config.h2 && !config.no_secure && !config.force_test_dc {
            if let Some(channel) = pools.h2.open(dc, &domains).await {
                return channel.bridge(client, session).await;
            }
        }
        for base in domains {
            let host = format!("kws{dc}.{base}");
            if let Ok(ws) = ws_open(
                &host,
                &host,
                &host,
                "/apiws",
                !config.no_secure,
                config.buffer_size,
            )
            .await
            {
                pools.preferred.lock().unwrap().insert(dc, base);
                return via_ws(client, session, ws).await;
            }
        }
    }
    let dest = dest.ok_or_else(|| ioe("Для этого DC нет маршрута Telegram."))?;
    if !pools.available(dest) || !pools.tcp_connecting.lock().unwrap().insert(dest.into()) {
        return Err(ioe("Повторное подключение Telegram отложено."));
    }
    struct Attempt<'a>(&'a Mutex<std::collections::HashSet<String>>, &'a str);
    impl Drop for Attempt<'_> {
        fn drop(&mut self) {
            self.0.lock().unwrap().remove(self.1);
        }
    }
    let attempt = Attempt(&pools.tcp_connecting, dest);
    let remote = timeout(Duration::from_secs(10), TcpStream::connect((dest, 443)))
        .await
        .map_err(ioe)
        .and_then(|s| s);
    drop(attempt);
    pools.record(dest, remote.is_ok());
    via_tcp(client, session, remote?).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shipped_domains_are_valid() {
        assert!(
            include_str!("default-domains.txt")
                .lines()
                .all(valid_domain)
        );
        assert_eq!(
            decode_domain("virkgj.com"),
            include_str!("default-domains.txt").lines().next().unwrap()
        );
    }
    #[test]
    fn only_telegram_dc_tcp_targets() {
        assert!(dc_ip(255, false).is_none());
        assert!(dc_ip(4, true).is_none());
        assert!(dc_ip(2, false).unwrap().parse::<std::net::IpAddr>().is_ok());
    }
}

#[cfg(test)]
mod websocket_roundtrip {
    use super::*;
    use crate::protocol::Cipher;
    use aes::cipher::KeyIvInit;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn retries_only_when_the_server_omits_the_optional_binary_subprotocol() {
        use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
        for (protocol, good_accept, requests, success) in [
            (Some("binary"), true, 1, true),
            (None, true, 2, true),
            (Some("text"), true, 1, false),
            (None, false, 1, false),
        ] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = ("127.0.0.1", listener.local_addr().unwrap().port());
            let peer = async {
                for attempt in 0..requests {
                    let (socket, _) = listener.accept().await.unwrap();
                    tokio_tungstenite::accept_hdr_async(
                        socket,
                        |req: &Request, mut resp: Response| {
                            assert_eq!(req.uri().path(), "/apiws");
                            assert_eq!(
                                req.headers().get("Sec-WebSocket-Protocol").is_some(),
                                attempt == 0
                            );
                            if let Some(name) = protocol {
                                resp.headers_mut()
                                    .insert("Sec-WebSocket-Protocol", name.parse().unwrap());
                            }
                            if !good_accept {
                                resp.headers_mut()
                                    .insert("Sec-WebSocket-Accept", "mismatch".parse().unwrap());
                            }
                            Ok(resp)
                        },
                    )
                    .await
                    .unwrap();
                }
            };
            let open = ws_open_at(address, "app.local", "app.local", "/apiws", false, 16_384);
            let (result, ()) = timeout(Duration::from_secs(2), async { tokio::join!(open, peer) })
                .await
                .unwrap();
            assert_eq!(result.is_ok(), success);
        }
    }

    fn cipher(seed: u8) -> Cipher {
        Cipher::new_from_slices(&[seed; 32], &[seed; 16]).unwrap()
    }

    #[tokio::test]
    async fn fragmented_client_frames_and_websocket_replies_are_reencrypted() {
        timeout(Duration::from_secs(5), async {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let peer = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut peer = tokio_tungstenite::accept_async(socket).await.unwrap();
                assert_eq!(
                    peer.next().await.unwrap().unwrap().into_data().as_ref(),
                    &[9; 64]
                );
                let mut decode = cipher(3);
                for payload in [b"abcdefgh", b"ABCDEFGH"] {
                    let mut data = peer.next().await.unwrap().unwrap().into_data().to_vec();
                    decode.apply_keystream(&mut data);
                    assert_eq!(&data[..4], &8u32.to_le_bytes());
                    assert_eq!(&data[4..], payload);
                }
                let mut reply = b"answer from dc".to_vec();
                cipher(4).apply_keystream(&mut reply);
                peer.send(Message::Binary(reply.into())).await.unwrap();
                peer.close(None).await.unwrap();
            });
            let raw = MaybeTlsStream::Plain(TcpStream::connect(address).await.unwrap());
            let (ws, _) = tokio_tungstenite::client_async(format!("ws://{address}/apiws"), raw)
                .await
                .unwrap();
            let session = Session {
                dc: 2,
                media: false,
                test: false,
                tag: [0xee; 4],
                init: [9; 64],
                client_in: cipher(1),
                client_out: cipher(2),
                relay_in: cipher(4),
                relay_out: cipher(3),
            };
            let (mut client, server) = tokio::io::duplex(128);
            let relay = tokio::spawn(via_ws(server, session, ws));
            let mut cipher_in = cipher(1);
            for part in [b"abcdefgh", b"ABCDEFGH"] {
                let mut frame = 8u32.to_le_bytes().to_vec();
                frame.extend_from_slice(part);
                cipher_in.apply_keystream(&mut frame);
                for fragment in frame.chunks(3) {
                    client.write_all(fragment).await.unwrap();
                }
            }
            let mut received = [0; 14];
            client.read_exact(&mut received).await.unwrap();
            cipher(2).apply_keystream(&mut received);
            assert_eq!(&received, b"answer from dc");
            peer.await.unwrap();
            relay.await.unwrap().unwrap();
        })
        .await
        .unwrap();
    }
}

#[cfg(test)]
#[path = "tcp_roundtrip.rs"]
mod tcp_roundtrip;
