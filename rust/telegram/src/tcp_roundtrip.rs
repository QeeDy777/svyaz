use crate::{fake_tls, protocol};
use std::{io, net::Ipv4Addr, time::Duration};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

#[tokio::test]
async fn authenticated_client_receives_a_tcp_dc_response_while_both_peers_are_open()
-> io::Result<()> {
    let key = [0x2d; 16];
    let incoming = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let dc = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = incoming.local_addr()?;
    let dc_addr = dc.local_addr()?;
    let (received, completed) = oneshot::channel();
    let relay = async {
        let (socket, _) = incoming.accept().await?;
        let mut stream = fake_tls::accept(socket, Some("app.local"), &key).await?;
        let mut header = [0; 64];
        stream.read_exact(&mut header).await?;
        let session = protocol::open(header, &key, false)?;
        let upstream = TcpStream::connect(dc_addr).await?;
        super::via_tcp(stream, session, upstream).await
    };
    let destination = async {
        let (mut peer, _) = dc.accept().await?;
        protocol::tests::respond_to_probe(&mut peer, None, true).await;
        completed.await.map_err(io::Error::other)
    };
    let client = async {
        let socket = TcpStream::connect(addr).await?;
        let stream = fake_tls::client(socket, Some("app.local"), &key).await?;
        protocol::probe(stream, &key, 2).await?;
        received
            .send(())
            .map_err(|_| io::Error::other("DC disconnected"))
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        tokio::try_join!(client, relay, destination)
    })
    .await??;
    Ok(())
}
