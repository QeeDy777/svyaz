use std::{
    fs,
    io::{self, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::atomic::AtomicBool,
};
use svyaz_settings::AppState;
use svyaz_telegram::{Runner, ensure_secret};

#[test]
fn native_engine_starts_twice_and_releases_its_listener() -> io::Result<()> {
    let data = tempfile::tempdir()?;
    let dir = data.path().join("Telegram/TgWsProxy_data");
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("config.json"),
        br#"{"pool_size":0,"cfproxy":false}"#,
    )?;
    let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = occupied.local_addr()?.port();
    let mut state = AppState::default();
    state.telegram.port = i32::from(port);
    ensure_secret(&mut state.telegram)?;
    let mut runner = Runner::default();
    let active = AtomicBool::new(false);
    let cancelled = AtomicBool::new(true);
    assert!(runner.start(data.path(), &state, &active).is_err());
    assert!(!runner.status()?.alive);
    drop(occupied);
    for _ in 0..2 {
        runner.start(data.path(), &state, &active)?;
        assert!(runner.status()?.ready);
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
        client.write_all(&[0; 64])?;
        drop(client);
        assert_eq!(
            runner.probe(&cancelled).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert!(runner.status()?.alive);
        runner.stop()?;
        assert!(!runner.status()?.alive);
        drop(TcpListener::bind((Ipv4Addr::LOCALHOST, port))?);
    }
    assert_eq!(
        runner
            .start(data.path(), &state, &cancelled)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    Ok(())
}
