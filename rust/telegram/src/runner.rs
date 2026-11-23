use crate::{config, fake_tls, protocol, runtime_config::Config, transport};
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
        self.stop()?;
        cancelled(cancel)?;
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
    pub fn probe(&self, cancel: &AtomicBool) -> io::Result<Option<u64>> {
        cancelled(cancel)?;
        let Some(service) = self.service.as_ref().filter(|s| !s.task.is_finished()) else {
            return Ok(None);
        };
        let cfg = &service.config;
        let start = Instant::now();
        let dc = if cfg.dc_redirects.contains_key(&2) {
            2
        } else {
            cfg.dc_redirects.keys().copied().min().unwrap_or(2)
        };
        service.rt.block_on(async{
            let request=async {
                let mut stream=TcpStream::connect((Ipv4Addr::LOCALHOST,cfg.port)).await?;
                if cfg.proxy_protocol{stream.write_all(b"PROXY UNKNOWN\r\n").await?;stream.flush().await?;}
                let stream = fake_tls::client(stream,cfg.fake_tls_domain.as_deref(),&cfg.secret).await?;
                protocol::probe(stream,&cfg.secret,i16::try_from(dc).map_err(io::Error::other)?).await
            };
            tokio::select!{
                res=tokio::time::timeout(Duration::from_secs(12),request)=>Ok(matches!(res,Ok(Ok(()))).then(||start.elapsed().as_millis() as u64)),
                _=async{while !cancel.load(Ordering::Acquire){tokio::time::sleep(Duration::from_millis(80)).await;}}=>Err(io::Error::new(io::ErrorKind::Interrupted,"Проверка Telegram отменена.")),
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
