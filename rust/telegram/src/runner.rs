use crate::{config, fake_tls, protocol, runtime_config::Config, transport};
use futures_util::{StreamExt, stream::FuturesUnordered};
use socket2::{Domain, Socket, Type};
use std::{
    io,
    net::{Ipv4Addr, SocketAddrV4},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use svyaz_settings::AppState;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    runtime::Runtime,
    task::{JoinHandle, JoinSet},
};

#[derive(Default)]
pub struct EngineState {
    pub alive: bool,
    pub ready: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Runner {
    service: Option<Service>,
}

#[derive(Debug)]
pub struct ProbeReply {
    pub dc: u16,
    pub elapsed_ms: u64,
}

struct Service {
    rt: Runtime,
    task: JoinHandle<io::Result<()>>,
    config: Arc<Config>,
}

fn cancelled(c: &AtomicBool) -> io::Result<()> {
    if c.load(Ordering::Acquire) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Операция Telegram отменена.",
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn check_external() -> io::Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND},
        System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE},
    };
    let name: Vec<u16> = "Local\\TgWsProxy_SingleInstance"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let handle = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr());
        if !handle.is_null() {
            CloseHandle(handle);
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Уже запущен отдельный TG WS Proxy. Завершите его через его трей и повторите запуск.",
            ));
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(ERROR_FILE_NOT_FOUND as i32) {
            return Err(e);
        }
    }
    Ok(())
}

fn listen(port: u16) -> io::Result<std::net::TcpListener> {
    let sock = Socket::new(Domain::IPV4, Type::STREAM, None)?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{
            SO_EXCLUSIVEADDRUSE, SOL_SOCKET, WSAGetLastError, setsockopt,
        };
        let value = 1_i32;
        if unsafe {
            setsockopt(
                sock.as_raw_socket() as _,
                SOL_SOCKET,
                SO_EXCLUSIVEADDRUSE,
                (&value as *const i32).cast(),
                4,
            )
        } != 0
        {
            return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
        }
    }
    sock.set_nonblocking(true)?;
    sock.bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into())
        .and_then(|_| sock.listen(128))
        .map_err(|e| {
            io::Error::new(
                e.kind(),
                "Порт Telegram занят или недоступен. Выберите другой порт.",
            )
        })?;
    Ok(sock.into())
}

async fn consume_proxy_header(client: &mut TcpStream) -> io::Result<()> {
    // The local listener trusts no address or routing claimed in a PROXY header.
    let mut line = Vec::with_capacity(108);
    while line.len() < 108 {
        line.push(client.read_u8().await?);
        if line.ends_with(b"\r\n") {
            break;
        }
    }
    if line.starts_with(b"PROXY ") && line.ends_with(b"\r\n") {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Неверный PROXY header.",
        ))
    }
}

async fn serve_client(
    mut socket: TcpStream,
    cfg: Arc<Config>,
    pools: Arc<transport::Pools>,
) -> io::Result<()> {
    socket.set_nodelay(true)?;
    let opts = socket2::SockRef::from(&socket);
    let _ = opts.set_send_buffer_size(cfg.buffer_size);
    let _ = opts.set_recv_buffer_size(cfg.buffer_size);
    if cfg.proxy_protocol {
        tokio::time::timeout(Duration::from_secs(10), consume_proxy_header(&mut socket)).await??;
    }
    let mut client = fake_tls::accept(socket, cfg.fake_tls_domain.as_deref(), &cfg.secret).await?;
    let mut header = [0; 64];
    tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut header)).await??;
    let session = protocol::open(header, &cfg.secret, cfg.force_test_dc)?;
    transport::serve(client, session, &cfg, &pools).await
}

async fn accept(
    listener: TcpListener,
    cfg: Arc<Config>,
    pools: Arc<transport::Pools>,
) -> io::Result<()> {
    let mut tasks = JoinSet::new();
    let warm = pools.clone();
    let warm_config = cfg.clone();
    tasks.spawn(async move {
        warm.warmup(&warm_config).await;
    });
    loop {
        tokio::select! {
            result=listener.accept()=>{
                match result{
                    Ok((socket,peer)) if peer.ip().is_loopback()=>{
                        let cfg=cfg.clone();let pools=pools.clone();
                        tasks.spawn(async move{let _=serve_client(socket,cfg,pools).await;});
                    }
                    Ok(_) => (),
                    Err(e) if matches!(e.kind(),io::ErrorKind::Interrupted|io::ErrorKind::ConnectionAborted)=>(),
                    Err(e)=>return Err(e),
                }
            }
            _=tasks.join_next(),if !tasks.is_empty()=>(),
        }
    }
}

fn probe_datacenters(cfg: &Config) -> Vec<u16> {
    let mut choices: Vec<_> = cfg
        .dc_redirects
        .keys()
        .copied()
        .filter(|dc| (1..=if cfg.force_test_dc { 3 } else { 5 }).contains(dc))
        .collect();
    if choices.is_empty() {
        choices.extend(if cfg.force_test_dc { [2, 1] } else { [2, 4] });
    }
    choices.sort_by_key(|dc| *dc != 2);
    choices.truncate(2);
    choices
}

async fn probe_dc(cfg: &Config, dc: u16) -> io::Result<ProbeReply> {
    let start = Instant::now();
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, cfg.port))
        .await
        .map_err(|e| io::Error::new(e.kind(), format!("Локальный прокси недоступен: {e}")))?;
    if cfg.proxy_protocol {
        stream.write_all(b"PROXY UNKNOWN\r\n").await?;
        stream.flush().await?;
    }
    let stream = fake_tls::client(stream, cfg.fake_tls_domain.as_deref(), &cfg.secret).await?;
    protocol::probe(stream, &cfg.secret, dc as i16)
        .await
        .map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                io::Error::new(e.kind(), "Соединение закрылось до ответа resPQ.")
            } else {
                e
            }
        })?;
    Ok(ProbeReply {
        dc,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

impl Runner {
    pub fn status(&self) -> io::Result<EngineState> {
        let alive = self.service.as_ref().is_some_and(|s| !s.task.is_finished());
        Ok(EngineState {
            alive,
            ready: alive,
            error: (self.service.is_some() && !alive)
                .then(|| "Локальный сервер остановился.".into()),
        })
    }
    pub fn start(
        &mut self,
        data_root: &Path,
        state: &AppState,
        cancel: &AtomicBool,
    ) -> io::Result<()> {
        cancelled(cancel)?;
        config::validate(&state.telegram)?;
        self.stop()?;
        #[cfg(windows)]
        check_external()?;
        let raw = config::configuration(&data_root.join("Telegram"), state)?;
        let cfg = Arc::new(Config::parse(&raw)?);
        let listener = listen(cfg.port)?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("svyaz-telegram")
            .enable_all()
            .build()?;
        let (socket, pool) = {
            let _enter = rt.enter();
            (
                TcpListener::from_std(listener)?,
                transport::Pools::new(&cfg)?,
            )
        };
        cancelled(cancel)?;
        let task = rt.spawn(accept(socket, cfg.clone(), Arc::new(pool)));
        self.service = Some(Service {
            rt,
            task,
            config: cfg,
        });
        Ok(())
    }
    pub fn probe(&self, cancel: &AtomicBool) -> io::Result<ProbeReply> {
        cancelled(cancel)?;
        let service = self
            .service
            .as_ref()
            .filter(|s| !s.task.is_finished())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotConnected,
                    "Локальный прокси Telegram остановлен.",
                )
            })?;
        let cfg = &service.config;
        service.rt.block_on(async {
            let mut pending = FuturesUnordered::new();
            for (index, dc) in probe_datacenters(cfg).into_iter().enumerate() {
                pending.push(async move {
                    // Let a working primary answer first and give cold route pools time to fill.
                    if index > 0 {
                        tokio::time::sleep(Duration::from_millis(1200)).await;
                    }
                    let result = tokio::time::timeout(Duration::from_secs(20), probe_dc(cfg, dc))
                        .await
                        .unwrap_or_else(|_| {
                            Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "Нет ответа за 20 с.",
                            ))
                        });
                    (dc, result)
                });
            }
            let exchanges = async {
                let mut errors = Vec::new();
                while let Some((dc, result)) = pending.next().await {
                    match result {
                        Ok(reply) => return Ok(reply),
                        Err(e) => errors.push(format!("DC{dc}: {e}")),
                    }
                }
                Err(io::Error::other(errors.join(" · ")))
            };
            tokio::select! {
                result = exchanges => result,
                _ = async {
                    while !cancel.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(80)).await;
                    }
                } => Err(io::Error::new(io::ErrorKind::Interrupted, "Проверка Telegram отменена.")),
            }
        })
    }
    pub fn stop(&mut self) -> io::Result<()> {
        if let Some(s) = self.service.take() {
            s.task.abort();
            let _ = s.rt.block_on(s.task);
            s.rt.shutdown_timeout(Duration::from_secs(2));
        }
        Ok(())
    }
}
impl Drop for Runner {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::tests::respond_to_probe;
    #[test]
    fn another_configured_dc_can_confirm_while_the_primary_stalls() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let listener = rt
            .block_on(TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))
            .unwrap();
        let cfg = Arc::new(
            Config::parse(
                &serde_json::json!({
                    "port":listener.local_addr().unwrap().port(),"secret":"4e".repeat(16),
                    "dc_ip":["2:127.0.0.1","4:127.0.0.1"]
                })
                .to_string(),
            )
            .unwrap(),
        );
        let key = cfg.secret;
        let task = rt.spawn(async move {
            let (_silent_primary, _) = listener.accept().await?;
            let (secondary, _) = listener.accept().await?;
            protocol::tests::respond_to_probe_for_dc(secondary, Some(&key), 4, true).await;
            Ok(())
        });
        let runner = Runner {
            service: Some(Service {
                rt,
                task,
                config: cfg,
            }),
        };
        assert_eq!(runner.probe(&AtomicBool::new(false)).unwrap().dc, 4);
    }

    #[test]
    fn local_probe_works_with_proxy_header_and_authenticated_tls() {
        for domain in ["", "app.local"] {
            for proxy_protocol in [true, false] {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let listener = runtime
                    .block_on(TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))
                    .unwrap();
                let cfg = serde_json::json!({
                    "port": listener.local_addr().unwrap().port(),
                    "secret":"2a".repeat(16),
                    "dc_ip":["2:127.0.0.1"],
                    "fake_tls_domain": domain,
                    "proxy_protocol": proxy_protocol,
                });
                let cfg = Arc::new(Config::parse(&cfg.to_string()).unwrap());
                let peer = Arc::clone(&cfg);
                let task = runtime.spawn(async move {
                    for correct in [true, false] {
                        let (mut stream, _) = listener.accept().await?;
                        if peer.proxy_protocol {
                            consume_proxy_header(&mut stream).await?;
                        }
                        let stream =
                            fake_tls::accept(stream, peer.fake_tls_domain.as_deref(), &peer.secret)
                                .await?;
                        respond_to_probe(stream, Some(&peer.secret), correct).await;
                    }
                    Ok(())
                });
                let runner = Runner {
                    service: Some(Service {
                        rt: runtime,
                        task,
                        config: cfg,
                    }),
                };
                let active = AtomicBool::new(false);
                assert_eq!(runner.probe(&active).unwrap().dc, 2);
                assert!(
                    runner
                        .probe(&active)
                        .unwrap_err()
                        .to_string()
                        .contains("nonce")
                );
                assert!(runner.service.as_ref().unwrap().task.is_finished());
            }
        }
    }
}
