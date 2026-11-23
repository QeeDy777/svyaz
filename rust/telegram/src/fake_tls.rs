//! Telegram's authenticated `ee` MTProxy record transport for the local listener.
//! A rejected handshake never opens an outbound socket.
use bytes::{Buf, BufMut, Bytes, BytesMut};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{
    collections::{HashSet, VecDeque},
    io,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::{
    codec::{Decoder, Encoder, Framed},
    io::{SinkWriter, StreamReader},
};

const RECORD_LIMIT: usize = 16_384;
const REPLAY_LIFE: Duration = Duration::from_secs(241);
const MAX_RECENT: usize = 65_536;
type Hmac256 = Hmac<Sha256>;
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Некорректный запрос локального прокси.",
    )
}
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Не удалось подтвердить запрос локального прокси.",
    )
}

#[derive(Default)]
struct ReplayCache {
    seen: HashSet<[u8; 32]>,
    ordered: VecDeque<(Instant, [u8; 32])>,
}
impl ReplayCache {
    fn accept(&mut self, nonce: [u8; 32], now: Instant) -> bool {
        while self
            .ordered
            .front()
            .is_some_and(|(expiry, _)| *expiry <= now)
        {
            let (_, old) = self.ordered.pop_front().unwrap();
            self.seen.remove(&old);
        }
        if self.seen.contains(&nonce) || self.seen.len() >= MAX_RECENT {
            return false;
        }
        self.seen.insert(nonce);
        self.ordered.push_back((now + REPLAY_LIFE, nonce));
        true
    }
}
static REPLAY: LazyLock<Mutex<ReplayCache>> = LazyLock::new(Default::default);
struct ClientHello {
    random: [u8; 32],
    session: [u8; 32],
    suite: u16,
    hybrid: bool,
}

fn take<'a>(buf: &mut &'a [u8], n: usize) -> io::Result<&'a [u8]> {
    let (part, rest) = buf.split_at_checked(n).ok_or_else(invalid)?;
    *buf = rest;
    Ok(part)
}
fn byte(buf: &mut &[u8]) -> io::Result<u8> {
    Ok(take(buf, 1)?[0])
}
fn word(buf: &mut &[u8]) -> io::Result<u16> {
    Ok(u16::from_be_bytes(take(buf, 2)?.try_into().unwrap()))
}
fn list<'a>(buf: &mut &'a [u8]) -> io::Result<&'a [u8]> {
    let n = word(buf)?;
    take(buf, n.into())
}
fn offered(body: &[u8]) -> io::Result<(u16, bool)> {
    let mut b = body;
    let session = usize::from(byte(&mut b)?);
    take(&mut b, session)?;
    let suites = list(&mut b)?;
    let preferred = [0x1301u16, 0x1303, 0x1302]
        .into_iter()
        .find(|s| suites.chunks_exact(2).any(|x| x == s.to_be_bytes()))
        .unwrap_or(0x1301);
    let n = usize::from(byte(&mut b)?);
    take(&mut b, n)?;
    let mut extensions = list(&mut b)?;
    let mut hybrid = false;
    while !extensions.is_empty() {
        let kind = word(&mut extensions)?;
        let mut data = list(&mut extensions)?;
        if kind == 0x0033 {
            let mut shares = list(&mut data)?;
            while !shares.is_empty() {
                let group = word(&mut shares)?;
                let key = list(&mut shares)?;
                hybrid |= group == 0x11ec && key.len() == 1216;
            }
        }
    }
    Ok((preferred, hybrid))
}
fn authenticate(bytes: &[u8], secret: &[u8; 16], now: u64) -> io::Result<ClientHello> {
    if bytes.len() < 43 || bytes[0] != 0x16 || bytes[5] != 1 {
        return Err(invalid());
    }
    let random: [u8; 32] = bytes[11..43].try_into().unwrap();
    let mut mac = Hmac256::new_from_slice(secret).expect("fixed key length");
    mac.update(&bytes[..11]);
    mac.update(&[0; 32]);
    mac.update(&bytes[43..]);
    let expected = mac.clone().finalize().into_bytes();
    mac.verify_truncated_left(&random[..28])
        .map_err(|_| denied())?;
    let mut ts = [0; 4];
    for i in 0..4 {
        ts[i] = expected[28 + i] ^ random[28 + i];
    }
    if now.abs_diff(u64::from(u32::from_le_bytes(ts))) > 120 {
        return Err(denied());
    }
    let mut session = [0; 32];
    if bytes.get(43) == Some(&32) {
        session.copy_from_slice(bytes.get(44..76).ok_or_else(invalid)?);
    }
    let (suite, hybrid) = offered(&bytes[43..]).unwrap_or((0x1301, false));
    Ok(ClientHello {
        random,
        session,
        suite,
        hybrid,
    })
}
fn record(out: &mut Vec<u8>, kind: u8, data: &[u8]) {
    out.extend_from_slice(&[kind, 3, 3]);
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
}
fn random(data: &mut [u8]) -> io::Result<()> {
    getrandom::fill(data).map_err(|_| io::Error::other("Недоступен генератор случайных чисел."))
}
fn reply(hello: &ClientHello, secret: &[u8; 16]) -> io::Result<Vec<u8>> {
    let mut private = [0; 32];
    random(&mut private)?;
    let key = x25519_dalek::x25519(private, x25519_dalek::X25519_BASEPOINT_BYTES);
    let mut share = Vec::new();
    if hello.hybrid {
        share.resize(1088, 0);
        random(&mut share)?;
    }
    share.extend_from_slice(&key);
    let group: u16 = if hello.hybrid { 0x11ec } else { 0x001d };
    let mut extensions = vec![0, 43, 0, 2, 3, 4, 0, 51];
    extensions.extend_from_slice(&((4 + share.len()) as u16).to_be_bytes());
    extensions.extend_from_slice(&group.to_be_bytes());
    extensions.extend_from_slice(&(share.len() as u16).to_be_bytes());
    extensions.extend_from_slice(&share);
    let mut body = vec![3, 3];
    body.extend_from_slice(&[0; 32]);
    body.push(32);
    body.extend_from_slice(&hello.session);
    body.extend_from_slice(&hello.suite.to_be_bytes());
    body.push(0);
    body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    body.extend(extensions);
    let mut handshake = vec![2];
    handshake.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    handshake.extend(body);
    let mut result = Vec::new();
    record(&mut result, 22, &handshake);
    record(&mut result, 20, &[1]);
    let mut length = [0];
    random(&mut length)?;
    let mut padding = vec![0; 64 + usize::from(length[0]) % 97];
    random(&mut padding)?;
    record(&mut result, 23, &padding);
    let mut mac = Hmac256::new_from_slice(secret).expect("fixed key length");
    mac.update(&hello.random);
    mac.update(&result);
    result[11..43].copy_from_slice(&mac.finalize().into_bytes());
    Ok(result)
}

struct RecordCodec {
    tls: bool,
}
impl Decoder for RecordCodec {
    type Item = Bytes;
    type Error = io::Error;
    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<Bytes>> {
        if !self.tls {
            return Ok((!src.is_empty()).then(|| src.split().freeze()));
        }
        while src.len() >= 5 {
            if src[1..3] != [3, 3] || !matches!(src[0], 20 | 23) {
                return Err(invalid());
            }
            let len = u16::from_be_bytes([src[3], src[4]]) as usize;
            if len > RECORD_LIMIT {
                return Err(invalid());
            }
            if src.len() < 5 + len {
                return Ok(None);
            }
            let kind = src[0];
            src.advance(5);
            let content = src.split_to(len).freeze();
            if kind == 20 {
                if content.as_ref() != [1] {
                    return Err(invalid());
                }
            } else if len != 0 {
                return Ok(Some(content));
            }
        }
        Ok(None)
    }
}
impl Encoder<&[u8]> for RecordCodec {
    type Error = io::Error;
    fn encode(&mut self, bytes: &[u8], out: &mut BytesMut) -> io::Result<()> {
        if !self.tls {
            out.extend_from_slice(bytes);
            return Ok(());
        }
        for chunk in bytes.chunks(RECORD_LIMIT) {
            out.extend_from_slice(&[23, 3, 3]);
            out.put_u16(chunk.len() as u16);
            out.extend_from_slice(chunk);
        }
        Ok(())
    }
}

pub(crate) async fn accept(
    mut stream: TcpStream,
    domain: Option<&str>,
    secret: &[u8; 16],
) -> io::Result<impl AsyncRead + AsyncWrite + Unpin + Send + 'static> {
    if !stream.peer_addr()?.ip().is_loopback() {
        return Err(denied());
    }
    let tls = domain.is_some_and(|d| !d.is_empty());
    if tls {
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut header = [0; 5];
            stream.read_exact(&mut header).await?;
            if header[0] != 22 || header[1] != 3 {
                return Err(invalid());
            }
            let length = u16::from_be_bytes([header[3], header[4]]) as usize;
            if length > RECORD_LIMIT {
                return Err(invalid());
            }
            let mut data = header.to_vec();
            data.resize(5 + length, 0);
            stream.read_exact(&mut data[5..]).await?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_secs();
            let hello = authenticate(&data, secret, now)?;
            if !REPLAY
                .lock()
                .map_err(|_| io::Error::other("Ошибка проверки запроса."))?
                .accept(hello.random, Instant::now())
            {
                return Err(denied());
            }
            stream.write_all(&reply(&hello, secret)?).await?;
            stream.flush().await
        })
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    }
    Ok(SinkWriter::new(StreamReader::new(Framed::new(
        stream,
        RecordCodec { tls },
    ))))
}

fn extension(out: &mut Vec<u8>, id: u16, data: &[u8]) {
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
}

fn client_hello(
    domain: &str,
    secret: &[u8; 16],
    timestamp: u32,
) -> io::Result<(Vec<u8>, ClientHello)> {
    if domain.len() > 253
        || domain.split('.').any(|s| {
            s.is_empty()
                || s.len() > 63
                || s.starts_with('-')
                || s.ends_with('-')
                || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(invalid());
    }
    let mut session = [0; 32];
    random(&mut session)?;
    let mut private = [0; 32];
    random(&mut private)?;
    let public = x25519_dalek::x25519(private, x25519_dalek::X25519_BASEPOINT_BYTES);
    let mut ext = Vec::new();
    let mut sni = Vec::new();
    sni.extend_from_slice(&((3 + domain.len()) as u16).to_be_bytes());
    sni.push(0);
    sni.extend_from_slice(&(domain.len() as u16).to_be_bytes());
    sni.extend_from_slice(domain.as_bytes());
    extension(&mut ext, 0, &sni);
    extension(&mut ext, 10, &[0, 2, 0, 29]);
    extension(&mut ext, 13, &[0, 4, 8, 4, 4, 3]);
    extension(&mut ext, 43, &[2, 3, 4]);
    let mut share = vec![0, 36, 0, 29, 0, 32];
    share.extend_from_slice(&public);
    extension(&mut ext, 51, &share);
    let mut body = vec![3, 3];
    body.extend_from_slice(&[0; 32]);
    body.push(32);
    body.extend_from_slice(&session);
    body.extend_from_slice(&[0, 6, 0x13, 1, 0x13, 3, 0x13, 2, 1, 0]);
    body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
    body.extend(ext);
    let mut handshake = vec![1];
    handshake.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    handshake.extend(body);
    let mut packet = Vec::new();
    record(&mut packet, 22, &handshake);
    packet[2] = 1;
    let mut mac = Hmac256::new_from_slice(secret).expect("fixed key length");
    mac.update(&packet);
    let mut nonce: [u8; 32] = mac.finalize().into_bytes().into();
    for (byte, time) in nonce[28..].iter_mut().zip(timestamp.to_le_bytes()) {
        *byte ^= time;
    }
    packet[11..43].copy_from_slice(&nonce);
    Ok((
        packet,
        ClientHello {
            random: nonce,
            session,
            suite: 0x1301,
            hybrid: false,
        },
    ))
}

async fn read_record(stream: &mut TcpStream, expected: u8, output: &mut Vec<u8>) -> io::Result<()> {
    let mut header = [0; 5];
    stream.read_exact(&mut header).await?;
    let length = u16::from_be_bytes([header[3], header[4]]) as usize;
    if header[..3] != [expected, 3, 3] || length == 0 || length > RECORD_LIMIT {
        return Err(invalid());
    }
    output.extend_from_slice(&header);
    let start = output.len();
    output.resize(start + length, 0);
    stream.read_exact(&mut output[start..]).await?;
    Ok(())
}

pub(crate) async fn client(
    mut stream: TcpStream,
    domain: Option<&str>,
    secret: &[u8; 16],
) -> io::Result<impl AsyncRead + AsyncWrite + Unpin + Send + 'static> {
    if !stream.peer_addr()?.ip().is_loopback() {
        return Err(denied());
    }
    let domain = domain.filter(|d| !d.is_empty());
    if let Some(name) = domain {
        tokio::time::timeout(Duration::from_secs(10), async {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_secs();
            let (packet, hello) =
                client_hello(name, secret, u32::try_from(now).map_err(|_| invalid())?)?;
            stream.write_all(&packet).await?;
            stream.flush().await?;
            let mut server = Vec::new();
            read_record(&mut stream, 22, &mut server).await?;
            if server.len() < 76
                || server[5] != 2
                || server[43] != 32
                || server[44..76] != hello.session
            {
                return Err(invalid());
            }
            let ccs = server.len();
            read_record(&mut stream, 20, &mut server).await?;
            if server[ccs..] != [20, 3, 3, 0, 1, 1] {
                return Err(invalid());
            }
            read_record(&mut stream, 23, &mut server).await?;
            let signature: [u8; 32] = server[11..43].try_into().unwrap();
            server[11..43].fill(0);
            let mut mac = Hmac256::new_from_slice(secret).expect("fixed key length");
            mac.update(&hello.random);
            mac.update(&server);
            mac.verify_slice(&signature).map_err(|_| denied())
        })
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    }
    Ok(SinkWriter::new(StreamReader::new(Framed::new(
        stream,
        RecordCodec {
            tls: domain.is_some(),
        },
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    const TIME: u64 = 1_700_000_000;
    fn fixture() -> (Vec<u8>, [u8; 16]) {
        // Fixed hello authenticated by Python hmac.new(..., hashlib.sha256).
        let hex = "16030100510100004d0303f270f3961ca666a28174b81f93d8193d80cedaf0def6ce8fedcdcae028fb5f7e20000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f00041303130201000000";
        let bytes = hex
            .as_bytes()
            .chunks_exact(2)
            .map(|digits| u8::from_str_radix(std::str::from_utf8(digits).unwrap(), 16).unwrap())
            .collect();
        (bytes, std::array::from_fn(|n| n as u8))
    }
    #[test]
    fn local_authenticated_roundtrip_with_real_client_probe() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let listen = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let addr = listen.local_addr().unwrap();
            let secret = [17; 16];
            let server = tokio::spawn(async move {
                let (tcp, _) = listen.accept().await.unwrap();
                let mut net = accept(tcp, Some("example.org"), &secret).await.unwrap();
                let mut input = vec![0; 45_000];
                net.read_exact(&mut input).await.unwrap();
                assert!(input.iter().all(|b| *b == 42));
                net.write_all(b"ok").await.unwrap();
                net.flush().await.unwrap();
            });
            let mut probe = client(
                TcpStream::connect(addr).await.unwrap(),
                Some("example.org"),
                &secret,
            )
            .await
            .unwrap();
            probe.write_all(&vec![42; 45_000]).await.unwrap();
            probe.flush().await.unwrap();
            let mut result = [0; 2];
            probe.read_exact(&mut result).await.unwrap();
            assert_eq!(&result, b"ok");
            server.await.unwrap();
        });
    }
    #[test]
    fn authenticated_client_and_server_records() {
        let (data, key) = fixture();
        let h = authenticate(&data, &key, TIME).unwrap();
        assert_eq!(h.suite, 0x1303);
        assert_eq!(h.session, std::array::from_fn(|n| n as u8));
        let mut response = reply(&h, &key).unwrap();
        let random = response[11..43].to_vec();
        response[11..43].fill(0);
        let mut mac = Hmac256::new_from_slice(&key).unwrap();
        mac.update(&h.random);
        mac.update(&response);
        mac.verify_slice(&random).unwrap();
        let end = 5 + usize::from(u16::from_be_bytes([response[3], response[4]]));
        assert_eq!(&response[end..end + 6], &[20, 3, 3, 0, 1, 1]);
        assert_eq!(response[end + 6], 23);
        assert!(authenticate(&data, &key, TIME + 121).is_err());
        assert!(authenticate(&data, &[0; 16], TIME).is_err());
        let mut altered = data;
        altered[53] ^= 1;
        assert!(authenticate(&altered, &key, TIME).is_err());
    }
    #[test]
    fn replay_blocked_through_window() {
        let now = Instant::now();
        let mut cache = ReplayCache::default();
        assert!(cache.accept([0; 32], now));
        assert!(!cache.accept([0; 32], now + Duration::from_secs(240)));
        assert!(cache.accept([0; 32], now + REPLAY_LIFE));
    }
    #[test]
    fn fragmented_records_and_control_frame() {
        let mut codec = RecordCodec { tls: true };
        let mut wire = BytesMut::from(&[20, 3, 3, 0, 1, 1][..]);
        let bytes = vec![27u8; RECORD_LIMIT + 27];
        codec.encode(&bytes, &mut wire).unwrap();
        let mut fragment = wire.split_to(50);
        assert!(codec.decode(&mut fragment).unwrap().is_none());
        fragment.extend(wire);
        assert_eq!(
            codec.decode(&mut fragment).unwrap().unwrap().len(),
            RECORD_LIMIT
        );
        assert_eq!(
            codec.decode(&mut fragment).unwrap().unwrap().as_ref(),
            &[27; 27]
        );
        assert!(fragment.is_empty());
        assert!(
            codec
                .decode(&mut BytesMut::from(&[23, 3, 3, 255, 255][..]))
                .is_err()
        );
    }
}
