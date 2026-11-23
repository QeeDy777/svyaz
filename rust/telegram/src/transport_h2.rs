//! HTTP/2 media fallback for Flowseal's MTProto HTTP endpoint.
use crate::protocol::Session;
use aes::cipher::StreamCipher;
use reqwest::{Client, StatusCode, Version, redirect::Policy};
use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::{
        Arc, Mutex as SyncMutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc},
    task::{AbortHandle, Id, JoinSet},
};

const MAX_PACKET: usize = 4 * 1024 * 1024;
const MAX_CHANNEL: usize = 8 * 1024 * 1024;
const MAX_LANE: usize = 32 * 1024 * 1024;
const SETUP: Duration = Duration::from_secs(8);
const TIMEOUT: Duration = Duration::from_secs(65);
const COOLDOWN: Duration = Duration::from_secs(30);
const REPLAY_LIMIT: usize = 64 * 1024;
const SECURE: [u8; 4] = [0xdd; 4];
const ABRIDGED: [u8; 4] = [0xef; 4];
fn invalid(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}

pub(crate) struct H2Pool {
    client: Client,
    hosts: SyncMutex<HashMap<String, Arc<Mutex<Host>>>>,
    dc_cooldown: SyncMutex<HashMap<u16, Instant>>,
}
#[derive(Default)]
struct Host {
    lane: Option<Arc<Lane>>,
    retry_after: Option<Instant>,
}
struct Lane {
    client: Client,
    url: String,
    slots: Arc<Semaphore>,
    up: Arc<Semaphore>,
    down: Arc<Semaphore>,
    failed: SyncMutex<Option<Instant>>,
    active: SyncMutex<Vec<Weak<Active>>>,
}
pub(crate) struct H2Channel {
    lane: Arc<Lane>,
}
impl H2Pool {
    pub(crate) fn new() -> io::Result<Self> {
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .https_only(true)
            .http2_prior_knowledge()
            .http2_initial_stream_window_size(256 * 1024)
            .http2_initial_connection_window_size(4 * 1024 * 1024)
            .connect_timeout(SETUP)
            .timeout(TIMEOUT)
            .pool_max_idle_per_host(1)
            .build()
            .map_err(io::Error::other)?;
        Ok(Self {
            client,
            hosts: Default::default(),
            dc_cooldown: Default::default(),
        })
    }
    pub(crate) async fn open(&self, dc: u16, domains: &[String]) -> Option<H2Channel> {
        if !matches!(dc, 1..=5 | 203)
            || self
                .dc_cooldown
                .lock()
                .unwrap()
                .get(&dc)
                .is_some_and(|t| Instant::now() < *t)
        {
            return None;
        }
        let result = tokio::time::timeout(SETUP, async {
            for base in domains {
                if !valid_domain(base) {
                    continue;
                }
                let host = format!("kws{dc}.{base}");
                let entry = self
                    .hosts
                    .lock()
                    .unwrap()
                    .entry(host.clone())
                    .or_default()
                    .clone();
                let mut state = entry.lock().await;
                if state.retry_after.is_some_and(|t| Instant::now() < t) {
                    continue;
                }
                if let Some(lane) = &state.lane {
                    if lane
                        .failed
                        .lock()
                        .unwrap()
                        .is_some_and(|t| Instant::now() < t)
                    {
                        continue;
                    }
                    return Some(H2Channel { lane: lane.clone() });
                }
                let url = format!("https://{host}/api");
                let ready = self.client.head(&url).send().await.is_ok_and(|r| {
                    r.version() == Version::HTTP_2
                        && (r.status().is_success()
                            || matches!(
                                r.status(),
                                StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED
                            ))
                });
                if !ready {
                    state.retry_after = Some(Instant::now() + COOLDOWN);
                    continue;
                }
                let lane = Arc::new(Lane {
                    url,
                    client: self.client.clone(),
                    slots: Arc::new(Semaphore::new(64)),
                    up: Arc::new(Semaphore::new(MAX_LANE)),
                    down: Arc::new(Semaphore::new(MAX_LANE)),
                    failed: Default::default(),
                    active: Default::default(),
                });
                state.lane = Some(lane.clone());
                return Some(H2Channel { lane });
            }
            None
        })
        .await;
        result.unwrap_or_else(|_| {
            self.dc_cooldown
                .lock()
                .unwrap()
                .insert(dc, Instant::now() + COOLDOWN);
            None
        })
    }
}
fn valid_domain(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 248
        && s.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.as_bytes()[0].is_ascii_alphanumeric()
                && l.as_bytes()[l.len() - 1].is_ascii_alphanumeric()
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

async fn read_native<R: AsyncRead + Unpin>(
    reader: &mut R,
    crypt: &mut impl FnMut(&mut [u8]),
    tag: [u8; 4],
) -> io::Result<Vec<u8>> {
    let mut header = [0; 4];
    let length = if tag == ABRIDGED {
        reader.read_exact(&mut header[..1]).await?;
        crypt(&mut header[..1]);
        let first = header[0] & 0x7f;
        if first == 0x7f {
            header.fill(0);
            reader.read_exact(&mut header[..3]).await?;
            crypt(&mut header[..3]);
            u32::from_le_bytes(header) as usize * 4
        } else {
            first as usize * 4
        }
    } else {
        reader.read_exact(&mut header).await?;
        crypt(&mut header);
        (u32::from_le_bytes(header) & 0x7fffffff) as usize
    };
    if !(24..=MAX_PACKET + if tag == SECURE { 256 } else { 0 }).contains(&length) {
        return Err(invalid("Недопустимый размер пакета Telegram."));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    crypt(&mut body);
    if tag == SECURE {
        let clear = body[..8] == [0; 8];
        let packet = if clear {
            20u64 + u32::from_le_bytes(body[16..20].try_into().unwrap()) as u64
        } else {
            24 + ((length as u64 - 24) / 16) * 16
        };
        if packet < 24
            || packet > length as u64
            || length as u64 - packet > if clear { 256 } else { 15 }
        {
            return Err(invalid("Неверное дополнение пакета Telegram."));
        }
        body.truncate(packet as usize);
    }
    if body.len() % 4 != 0 || body.len() > MAX_PACKET {
        return Err(invalid("Неверное выравнивание пакета Telegram."));
    }
    Ok(body)
}

fn encode_reply(body: &[u8], tag: [u8; 4]) -> io::Result<Vec<u8>> {
    if body.is_empty() || body.len() % 4 != 0 || body.len() > MAX_PACKET {
        return Err(invalid("Неверный ответ Telegram."));
    }
    let mut bytes = Vec::with_capacity(body.len() + 7);
    if tag == ABRIDGED {
        let words = (body.len() / 4) as u32;
        if words < 0x7f {
            bytes.push(words as u8);
        } else {
            bytes.push(0x7f);
            bytes.extend_from_slice(&words.to_le_bytes()[..3]);
        }
        bytes.extend_from_slice(body);
    } else {
        let mut random = [0u8; 4];
        if tag == SECURE {
            getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
        }
        let padding = (random[0] % 4) as usize;
        bytes.extend_from_slice(&((body.len() + padding) as u32).to_le_bytes());
        bytes.extend_from_slice(body);
        bytes.extend_from_slice(&random[1..1 + padding]);
    }
    Ok(bytes)
}

enum Reply {
    Body(Vec<u8>),
    Error(i32),
}
struct Buffered {
    reply: Reply,
    _lane: Option<OwnedSemaphorePermit>,
    _channel: Option<OwnedSemaphorePermit>,
}
#[derive(Default)]
struct Progress {
    sent: SyncMutex<Option<Instant>>,
    headers: AtomicBool,
}
impl Progress {
    fn awaiting(&self, age: Duration) -> bool {
        !self.headers.load(Ordering::Acquire)
            && self
                .sent
                .lock()
                .unwrap()
                .is_some_and(|t| t.elapsed() >= age)
    }
}
fn reserve(mem: &Arc<Semaphore>, amount: usize) -> io::Result<OwnedSemaphorePermit> {
    mem.clone()
        .try_acquire_many_owned(amount as u32)
        .map_err(|_| io::Error::other("Буфер ответов Telegram заполнен."))
}
fn merge(target: &mut Option<OwnedSemaphorePermit>, more: OwnedSemaphorePermit) {
    if let Some(p) = target {
        p.merge(more)
    } else {
        *target = Some(more);
    }
}
impl Lane {
    fn reserve_upload(&self, n: usize) -> Option<(OwnedSemaphorePermit, OwnedSemaphorePermit)> {
        Some((
            self.slots.clone().try_acquire_owned().ok()?,
            self.up.clone().try_acquire_many_owned(n as u32).ok()?,
        ))
    }
    fn register(&self, flight: &Arc<Active>) {
        let mut active = self.active.lock().unwrap();
        active.retain(|a| a.strong_count() > 0);
        active.push(Arc::downgrade(flight));
    }
    async fn post(
        &self,
        body: Vec<u8>,
        progress: &Progress,
        reply_mem: Arc<Semaphore>,
    ) -> io::Result<Buffered> {
        *progress.sent.lock().unwrap() = Some(Instant::now());
        let mut response = self
            .client
            .post(&self.url)
            .header("content-type", "application/octet-stream")
            .header("accept-encoding", "identity")
            .body(body)
            .send()
            .await
            .map_err(io::Error::other)?;
        progress.headers.store(true, Ordering::Release);
        if response.version() != Version::HTTP_2 {
            return Err(invalid("Ожидался HTTP/2."));
        }
        let code = response.status().as_u16();
        let mut buffered = Buffered {
            reply: Reply::Body(Vec::new()),
            _lane: None,
            _channel: None,
        };
        if matches!(code, 403 | 404 | 429 | 444) {
            buffered.reply = Reply::Error(-i32::from(code));
            return Ok(buffered);
        }
        if code != 200
            || response
                .headers()
                .get("content-type")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h.to_ascii_lowercase().contains("text/html"))
        {
            return Err(invalid("Узел не вернул MTProto-пакет."));
        }
        let declared = response.content_length();
        if declared.is_some_and(|n| n > MAX_PACKET as u64) {
            return Err(invalid("Ответ Telegram слишком велик."));
        }
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(io::Error::other)? {
            if data.len() + chunk.len() > MAX_PACKET {
                return Err(invalid("Ответ Telegram слишком велик."));
            }
            merge(&mut buffered._lane, reserve(&self.down, chunk.len())?);
            merge(&mut buffered._channel, reserve(&reply_mem, chunk.len())?);
            data.extend_from_slice(&chunk);
        }
        if declared.is_some_and(|n| n != data.len() as u64) || data.len() % 4 != 0 {
            return Err(invalid("Неполный ответ Telegram."));
        }
        buffered.reply = match data.as_slice().try_into().map(i32::from_le_bytes) {
            Ok(code) if code < 0 => Reply::Error(code),
            _ => Reply::Body(data),
        };
        Ok(buffered)
    }
}

struct History {
    key: [u8; 8],
    entries: VecDeque<Remembered>,
}
struct Remembered {
    seq: u64,
    body: Vec<u8>,
    since: Instant,
    attempts: u8,
    last: Option<Instant>,
    reply_prefix: Vec<u8>,
    retired: bool,
    retained: Arc<()>,
}
struct Active {
    seq: u64,
    replay: bool,
    len: usize,
    progress: Arc<Progress>,
    handle: AbortHandle,
    retained: Option<(Weak<()>, Instant)>,
}
fn rotate(candidates: impl Iterator<Item = Arc<Active>>) {
    let oldest = candidates
        .filter(|a| {
            a.progress.awaiting(Duration::from_secs(3))
                && !a.handle.is_finished()
                && (a.replay
                    || a.retained.as_ref().is_some_and(|(entry, start)| {
                        entry.strong_count() > 0 && start.elapsed() <= COOLDOWN
                    }))
        })
        .min_by_key(|a| (!a.replay, *a.progress.sent.lock().unwrap()));
    if let Some(flight) = oldest {
        flight.handle.abort();
    }
}
impl History {
    fn new() -> Self {
        Self {
            key: [0; 8],
            entries: VecDeque::new(),
        }
    }
    fn remember(&mut self, seq: u64, body: &[u8], active: &HashMap<Id, Arc<Active>>) -> u64 {
        let key: [u8; 8] = body[..8].try_into().unwrap();
        if key == [0; 8] {
            return seq;
        }
        if key != self.key {
            self.entries.clear();
            self.key = key;
        }
        if body.len() > REPLAY_LIMIT {
            return seq;
        }
        if let Some(entry) = self.entries.iter().find(|e| e.body == body) {
            return entry.seq;
        }
        self.entries.push_back(Remembered {
            seq,
            body: body.into(),
            since: Instant::now(),
            attempts: 0,
            last: None,
            reply_prefix: Vec::new(),
            retired: false,
            retained: Arc::new(()),
        });
        while self.entries.len() > 16
            || self.entries.iter().map(|e| e.body.len()).sum::<usize>() > REPLAY_LIMIT
        {
            let index = self
                .entries
                .iter()
                .position(|e| {
                    !active.values().any(|a| a.seq == e.seq && !a.replay)
                        && !(e.retired && e.attempts < 3 && e.since.elapsed() <= COOLDOWN)
                })
                .unwrap_or(0);
            self.entries.remove(index);
        }
        seq
    }
    fn eligible(
        &self,
        entry: &Remembered,
        active: &HashMap<Id, Arc<Active>>,
        last_progress: Instant,
    ) -> Option<bool> {
        if entry.since.elapsed() > COOLDOWN
            || entry.attempts >= 3
            || entry
                .last
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return None;
        }
        let mut overdue = entry.retired;
        for a in active.values().filter(|a| a.seq == entry.seq) {
            if a.progress.headers.load(Ordering::Acquire)
                || a.replay && !a.progress.awaiting(COOLDOWN)
            {
                return None;
            }
            overdue |= !a.replay && a.progress.awaiting(Duration::from_secs(3));
        }
        if overdue {
            return Some(true);
        }
        if self.entries.back()?.seq != entry.seq
            || active.values().any(|a| a.seq != entry.seq || !a.replay)
            || entry.last.is_some() && last_progress.elapsed() < Duration::from_secs(1)
        {
            return None;
        }
        Some(false)
    }
    fn candidate(&self, active: &HashMap<Id, Arc<Active>>, last_progress: Instant) -> Option<u64> {
        let mut ready = None;
        for e in &self.entries {
            match self.eligible(e, active, last_progress) {
                Some(true) => return Some(e.seq),
                Some(false) => ready = Some(e.seq),
                None => {}
            }
        }
        ready
    }
}
struct ChannelRequests {
    lane: Arc<Lane>,
    tasks: JoinSet<io::Result<Buffered>>,
    active: HashMap<Id, Arc<Active>>,
    mem: Arc<Semaphore>,
    history: History,
    sequence: u64,
    last_progress: Instant,
}
impl ChannelRequests {
    fn new(lane: Arc<Lane>) -> Self {
        Self {
            lane,
            tasks: JoinSet::new(),
            active: HashMap::new(),
            mem: Arc::new(Semaphore::new(MAX_CHANNEL)),
            history: History::new(),
            sequence: 0,
            last_progress: Instant::now(),
        }
    }
    fn has_space(&self, n: usize) -> bool {
        self.active.len() < 8
            && n + self.active.values().map(|a| a.len).sum::<usize>() <= MAX_CHANNEL
    }
    fn issue(
        &mut self,
        body: Vec<u8>,
        seq: u64,
        replay: bool,
        permits: (OwnedSemaphorePermit, OwnedSemaphorePermit),
    ) {
        let len = body.len();
        let lane = self.lane.clone();
        let mem = self.mem.clone();
        let progress = Arc::new(Progress::default());
        let status = progress.clone();
        let retained = self
            .history
            .entries
            .iter()
            .find(|e| e.seq == seq)
            .map(|e| (Arc::downgrade(&e.retained), e.since));
        let handle = self.tasks.spawn(async move {
            let _permits = permits;
            let result = tokio::time::timeout(TIMEOUT, lane.post(body, &status, mem))
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Тайм-аут HTTP/2 Telegram."))
                .and_then(|x| x);
            if result.is_err() {
                *lane.failed.lock().unwrap() = Some(Instant::now() + COOLDOWN);
            }
            result
        });
        let flight = Arc::new(Active {
            seq,
            len,
            replay,
            progress,
            handle,
            retained,
        });
        self.lane.register(&flight);
        self.active.insert(flight.handle.id(), flight);
    }
    fn send(&mut self, body: Vec<u8>, permits: (OwnedSemaphorePermit, OwnedSemaphorePermit)) {
        self.sequence += 1;
        let seq = self.history.remember(self.sequence, &body, &self.active);
        self.issue(body, seq, false, permits);
    }
    fn recover(&mut self, waiting: Option<usize>) {
        if let Some(n) = waiting {
            if !self.has_space(n) {
                rotate(self.active.values().cloned());
                return;
            }
            if self.lane.slots.available_permits() == 0 || self.lane.up.available_permits() < n {
                rotate(
                    self.lane
                        .active
                        .lock()
                        .unwrap()
                        .iter()
                        .filter_map(Weak::upgrade),
                );
                return;
            }
        }
        let Some(seq) = self.history.candidate(&self.active, self.last_progress) else {
            return;
        };
        if let Some(a) = self.active.values().find(|a| a.replay && a.seq == seq) {
            a.handle.abort();
            return;
        }
        let running: Vec<_> = self.active.values().filter(|a| a.replay).collect();
        if running.len() >= 2 {
            if let Some(a) = running.into_iter().find(|a| a.progress.awaiting(COOLDOWN)) {
                a.handle.abort();
            }
            return;
        }
        let pos = self
            .history
            .entries
            .iter()
            .position(|e| e.seq == seq)
            .unwrap();
        let len = self.history.entries[pos].body.len();
        if !self.has_space(len) {
            return;
        }
        let Some(permits) = self.lane.reserve_upload(len) else {
            return;
        };
        let e = &mut self.history.entries[pos];
        e.last = Some(Instant::now());
        e.attempts += 1;
        let body = e.body.clone();
        self.issue(body, seq, true, permits);
    }
    async fn run<W: AsyncWrite + Unpin>(
        mut self,
        writer: &mut W,
        mut rx: mpsc::Receiver<Vec<u8>>,
        tag: [u8; 4],
        mut encrypt: impl FnMut(&mut [u8]),
    ) -> io::Result<()> {
        let mut waiting = None::<(Vec<u8>, Instant)>;
        let mut timer = tokio::time::interval(Duration::from_millis(250));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if let Some((body, queued)) = &waiting {
                if queued.elapsed() >= TIMEOUT {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Нет свободного канала HTTP/2.",
                    ));
                }
                if self.has_space(body.len()) {
                    if let Some(permits) = self.lane.reserve_upload(body.len()) {
                        self.send(waiting.take().unwrap().0, permits);
                    }
                }
            }
            tokio::select! {
                packet=rx.recv(),if waiting.is_none()=>match packet{Some(body)=>waiting=Some((body,Instant::now())),None=>return Ok(())},
                _=timer.tick()=> self.recover(waiting.as_ref().map(|(body,_)|body.len())),
                done=self.tasks.join_next_with_id(),if !self.active.is_empty()=>{
                    let Some(done)=done else{return Err(io::Error::other("HTTP/2 worker unexpectedly stopped."));};
                    let (id,outcome)=match done{
                        Err(e) if e.is_cancelled()=>{
                            if let Some(a)=self.active.remove(&e.id()){
                                if !a.replay{
                                    if let Some(entry)=self.history.entries.iter_mut().find(|entry|entry.seq==a.seq){entry.retired=true;}
                                }
                            }
                            continue;
                        },
                        Err(e)=>return Err(io::Error::other(e)),
                        Ok(done)=>done
                    };
                    let active=self.active.remove(&id).unwrap();
                    let item=outcome?;
                    self.last_progress=Instant::now();
                    let (body,transport_error)=match &item.reply {
                        Reply::Body(v) if v.is_empty()=>continue,
                        Reply::Body(v)=>(v.as_slice(),None),
                        Reply::Error(code)=>(&[][..],Some(*code)),
                    };
                    if let Some(e)=self.history.entries.iter_mut().find(|e|e.seq==active.seq){
                        let prefix=&body[..body.len().min(24)];
                        if prefix!=e.reply_prefix {e.attempts=0;e.reply_prefix=prefix.into();}
                    }
                    let mut wire=if let Some(code)=transport_error{encode_reply(&code.to_le_bytes(),tag)?}else{encode_reply(body,tag)?};
                    encrypt(&mut wire);
                    writer.write_all(&wire).await?;
                    writer.flush().await?;
                    self.last_progress=Instant::now();
                    if transport_error.is_some(){return Ok(());}
                }
            }
        }
    }
}

impl H2Channel {
    pub(crate) async fn bridge<S: AsyncRead + AsyncWrite + Unpin>(
        self,
        client: S,
        session: Session,
    ) -> io::Result<()> {
        let (mut reader, mut writer) = tokio::io::split(client);
        let tag = session.tag;
        let (mut decrypt, mut encrypt) = (session.client_in, session.client_out);
        let (tx, rx) = mpsc::channel(1);
        let upload = async {
            loop {
                let bytes =
                    read_native(&mut reader, &mut |buf| decrypt.apply_keystream(buf), tag).await?;
                tx.send(bytes)
                    .await
                    .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "HTTP/2 closed"))?;
            }
        };
        let broker = ChannelRequests::new(self.lane)
            .run(&mut writer, rx, tag, |buf| encrypt.apply_keystream(buf));
        tokio::select! {res=upload=>res,res=broker=>res}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, duplex},
        net::TcpListener,
    };
    fn plain() -> Vec<u8> {
        let mut body = vec![0; 28];
        body[16..20].copy_from_slice(&8u32.to_le_bytes());
        body[20..].copy_from_slice(b"telegram");
        body
    }
    #[tokio::test]
    async fn decodes_legacy_padded_clear_and_encrypted_frames() {
        let mut clear = plain();
        let mut wire = (0x80000000u32 | 31).to_le_bytes().to_vec();
        wire.extend_from_slice(&clear);
        wire.extend_from_slice(&[3, 2, 1]);
        assert_eq!(
            read_native(&mut &wire[..], &mut |_| {}, SECURE)
                .await
                .unwrap(),
            clear
        );
        let mut enc = vec![9; 40];
        enc.extend([1, 2, 3, 4]);
        let mut wire = (enc.len() as u32).to_le_bytes().to_vec();
        wire.extend_from_slice(&enc);
        assert_eq!(
            read_native(&mut &wire[..], &mut |_| {}, SECURE)
                .await
                .unwrap(),
            &enc[..40]
        );
        clear[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        let mut invalid = (clear.len() as u32).to_le_bytes().to_vec();
        invalid.extend(clear);
        assert!(
            read_native(&mut &invalid[..], &mut |_| {}, SECURE)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn abridged_extended_prefix_roundtrips() {
        for n in [24, 126 * 4, 127 * 4, MAX_PACKET] {
            let bytes = vec![7; n];
            let mut wire = encode_reply(&bytes, ABRIDGED).unwrap();
            if n >= 127 * 4 {
                assert_eq!(wire[0], 127);
            } else {
                assert_eq!(wire[0] as usize, n / 4);
            }
            wire[0] |= 128;
            assert_eq!(
                read_native(&mut &wire[..], &mut |_| {}, ABRIDGED)
                    .await
                    .unwrap(),
                bytes
            );
        }
        let too_large = ((MAX_PACKET + 4) as u32).to_le_bytes();
        assert_eq!(
            read_native(&mut &too_large[..], &mut |_| {}, [0xee; 4])
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(encode_reply(&[1, 2, 3], ABRIDGED).is_err());
    }
    #[test]
    fn recovery_is_bounded_and_never_replays_unencrypted_auth() {
        let mut h = History::new();
        let active = HashMap::new();
        h.remember(1, &plain(), &active);
        assert!(h.entries.is_empty());
        h.remember(2, &[1; 24], &active);
        h.remember(3, &[2; 24], &active);
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.candidate(&active, Instant::now()), Some(3));
        h.entries[0].attempts = 3;
        assert_eq!(h.candidate(&active, Instant::now()), None);
        assert!(valid_domain("example.org"));
        for bad in [
            ".domain",
            "a..b",
            "bad-url/other",
            "example.org@localhost",
            "a:8443",
            "-abc.net",
        ] {
            assert!(!valid_domain(bad));
        }
    }
    // Real HTTP/2 transactions on loopback exercise binary framing, concurrent requests and negative transport replies.
    #[tokio::test]
    async fn local_http2_bridges_upload_and_response() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut h = h2::server::handshake(stream).await.unwrap();
            let both = Arc::new(tokio::sync::Barrier::new(2));
            while let Some(req) = h.accept().await {
                let (req, mut tx) = req.unwrap();
                assert_eq!(req.method(), http::Method::POST);
                assert_eq!(req.uri().path(), "/api");
                let both = both.clone();
                tokio::spawn(async move {
                    let mut body = req.into_body();
                    let mut data = Vec::new();
                    while let Some(chunk) = body.data().await {
                        let chunk = chunk.unwrap();
                        data.extend_from_slice(&chunk);
                        body.flow_control().release_capacity(chunk.len()).unwrap();
                    }
                    if data[0] == 2 {
                        tx.send_response(
                            http::Response::builder().status(429).body(()).unwrap(),
                            true,
                        )
                        .unwrap();
                    } else {
                        both.wait().await;
                        let mut s = tx
                            .send_response(
                                http::Response::builder().status(200).body(()).unwrap(),
                                false,
                            )
                            .unwrap();
                        s.send_data(bytes::Bytes::from_static(b"MTProto!"), true)
                            .unwrap();
                    }
                });
            }
        });
        let test_client = Client::builder()
            .no_proxy()
            .http2_prior_knowledge()
            .build()
            .unwrap();
        let lane = Arc::new(Lane {
            client: test_client,
            url: format!("http://{addr}/api"),
            slots: Arc::new(Semaphore::new(64)),
            up: Arc::new(Semaphore::new(MAX_LANE)),
            down: Arc::new(Semaphore::new(MAX_LANE)),
            failed: Default::default(),
            active: Default::default(),
        });
        let broker = ChannelRequests::new(lane);
        let (mut rx, mut socket) = duplex(1024);
        let (tx, inbox) = mpsc::channel(1);
        let run =
            tokio::spawn(async move { broker.run(&mut socket, inbox, ABRIDGED, |_| {}).await });
        let mut a = plain();
        let mut b = a.clone();
        a[23] = 1;
        b[23] = 3;
        tx.send(a).await.unwrap();
        tx.send(b).await.unwrap();
        let mut first = [0; 18];
        tokio::time::timeout(Duration::from_secs(2), rx.read_exact(&mut first))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&first, b"\x02MTProto!\x02MTProto!");
        tx.send(vec![2; 24]).await.unwrap();
        let mut error = [0; 5];
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                rx.read_exact(&mut error[..1]).await.unwrap();
                let words = error[0];
                if words == 1 {
                    rx.read_exact(&mut error[1..]).await.unwrap();
                    break;
                }
                assert_eq!(words, 2);
                let mut duplicate = [0; 8];
                rx.read_exact(&mut duplicate).await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(i32::from_le_bytes(error[1..].try_into().unwrap()), -429);
        assert!(run.await.unwrap().is_ok());
        server.abort();
    }
}
