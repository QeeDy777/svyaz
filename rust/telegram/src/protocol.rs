//! Telegram obfuscated transport framing, using RustCrypto's AES-CTR primitive.
use aes::{
    Aes256,
    cipher::{KeyIvInit, StreamCipher},
};
use sha2::{Digest, Sha256};
use std::{
    io,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(crate) type Cipher = ctr::Ctr128BE<Aes256>;
const ABRIDGED: [u8; 4] = [0xef; 4];
const INTERMEDIATE: [u8; 4] = [0xee; 4];
const PADDED: [u8; 4] = [0xdd; 4];
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Неверный пакет Telegram.")
}
fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut value = [0; N];
    getrandom::fill(&mut value)
        .map_err(|_| io::Error::other("Генератор случайных чисел недоступен."))?;
    Ok(value)
}

pub(crate) struct Session {
    pub dc: u16,
    pub test: bool,
    pub media: bool,
    pub tag: [u8; 4],
    pub init: [u8; 64],
    pub client_in: Cipher,
    pub client_out: Cipher,
    pub relay_in: Cipher,
    pub relay_out: Cipher,
}

fn ciphers(header: &[u8; 64], secret: Option<&[u8; 16]>) -> (Cipher, Cipher) {
    let mut material: [u8; 48] = header[8..56].try_into().unwrap();
    let create = |parts: &[u8; 48]| {
        let key: [u8; 32] = match secret {
            Some(key) => Sha256::new()
                .chain_update(&parts[..32])
                .chain_update(key)
                .finalize()
                .into(),
            None => parts[..32].try_into().unwrap(),
        };
        Cipher::new_from_slices(&key, &parts[32..]).expect("fixed-size AES-256 key and CTR IV")
    };
    let forward = create(&material);
    material.reverse();
    (forward, create(&material))
}

fn generate_init(
    tag: [u8; 4],
    dc: i16,
    secret: Option<&[u8; 16]>,
) -> io::Result<([u8; 64], Cipher, Cipher)> {
    let mut header = loop {
        let h = random::<64>()?;
        if h[0] != 0xef
            && ![
                b"HEAD",
                b"POST",
                b"GET ",
                &INTERMEDIATE,
                &PADDED,
                b"\x16\x03\x01\x02",
                b"OPTI",
            ]
            .iter()
            .any(|prefix| h.starts_with(*prefix))
            && h[4..8] != [0; 4]
        {
            break h;
        }
    };
    let (mut forward, reverse) = ciphers(&header, secret);
    let mut encoded = header;
    encoded[56..60].copy_from_slice(&tag);
    encoded[60..62].copy_from_slice(&dc.to_le_bytes());
    forward.apply_keystream(&mut encoded);
    header[56..].copy_from_slice(&encoded[56..]);
    Ok((header, forward, reverse))
}

pub(crate) fn open(header: [u8; 64], secret: &[u8; 16], force_test: bool) -> io::Result<Session> {
    let (mut client_in, client_out) = ciphers(&header, Some(secret));
    let mut decoded = header;
    client_in.apply_keystream(&mut decoded);
    let tag = decoded[56..60].try_into().unwrap();
    if !matches!(tag, ABRIDGED | INTERMEDIATE | PADDED) {
        return Err(invalid());
    }
    let signed = i16::from_le_bytes(decoded[60..62].try_into().unwrap());
    let media = signed < 0;
    let raw = signed.unsigned_abs();
    let test = force_test || raw >= 10000;
    let dc = if raw >= 10000 { raw - 10000 } else { raw };
    let relay_dc = if media { -(dc as i16) } else { dc as i16 };
    let (init, relay_out, relay_in) = generate_init(tag, relay_dc, None)?;
    Ok(Session {
        dc,
        test,
        media,
        tag,
        init,
        client_in,
        client_out,
        relay_in,
        relay_out,
    })
}

/// Coalesced packets get separate WS messages; socket fragments stay buffered until complete.
pub(crate) struct Splitter {
    tag: [u8; 4],
    pending: Vec<u8>,
    passthrough: bool,
}
impl Splitter {
    pub fn new(tag: [u8; 4]) -> Self {
        Self {
            tag,
            pending: Vec::new(),
            passthrough: false,
        }
    }
    pub fn split(&mut self, plain: &[u8]) -> Vec<Vec<u8>> {
        if plain.is_empty() {
            return Vec::new();
        }
        if self.passthrough {
            return vec![plain.to_vec()];
        }
        self.pending.extend_from_slice(plain);
        let mut frames = vec![];
        let mut start = 0;
        while start < self.pending.len() {
            let p = &self.pending[start..];
            let (header, payload) = match self.tag {
                ABRIDGED if p[0] & 0x7f == 0x7f => {
                    if p.len() < 4 {
                        break;
                    }
                    (4, (u32::from_le_bytes([p[1], p[2], p[3], 0]) as usize) * 4)
                }
                ABRIDGED => (1, usize::from(p[0] & 0x7f) * 4),
                INTERMEDIATE | PADDED => {
                    if p.len() < 4 {
                        break;
                    }
                    (
                        4,
                        (u32::from_le_bytes(p[..4].try_into().unwrap()) & 0x7fff_ffff) as usize,
                    )
                }
                _ => (0, 0),
            };
            // Upstream falls back to streaming malformed frames. Bound the amount retained per client.
            if payload == 0 || payload > 16 * 1024 * 1024 {
                self.passthrough = true;
                frames.push(p.to_vec());
                start = self.pending.len();
                break;
            }
            if header + payload > p.len() {
                break;
            }
            frames.push(p[..header + payload].to_vec());
            start += header + payload;
        }
        self.pending.drain(..start);
        frames
    }
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        (!self.pending.is_empty()).then(|| std::mem::take(&mut self.pending))
    }
}

pub(crate) async fn probe(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    secret: &[u8; 16],
    dc: i16,
) -> io::Result<()> {
    let (init, mut outgoing, mut incoming) = generate_init(PADDED, dc, Some(secret))?;
    let nonce = random::<16>()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?;
    let id = ((now.as_secs() << 32) + ((u64::from(now.subsec_nanos()) << 32) / 1_000_000_000)) & !3;
    let pad = random::<16>()?;
    let n = usize::from(pad[15] & 15);
    let mut frame = Vec::with_capacity(59);
    frame.extend_from_slice(&((40 + n) as u32).to_le_bytes());
    frame.extend_from_slice(&[0; 8]);
    frame.extend_from_slice(&id.to_le_bytes());
    frame.extend_from_slice(&20u32.to_le_bytes());
    frame.extend_from_slice(&0xbe7e8ef1u32.to_le_bytes());
    frame.extend_from_slice(&nonce);
    frame.extend_from_slice(&pad[..n]);
    outgoing.apply_keystream(&mut frame);
    stream.write_all(&init).await?;
    stream.write_all(&frame).await?;
    stream.flush().await?;
    let mut len = [0; 4];
    stream.read_exact(&mut len).await?;
    incoming.apply_keystream(&mut len);
    let length = u32::from_le_bytes(len) as usize;
    if !(4..=4096).contains(&length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Неверная длина ответа MTProto.",
        ));
    }
    let mut reply = vec![0; length];
    stream.read_exact(&mut reply).await?;
    incoming.apply_keystream(&mut reply);
    if reply.len() < 20 {
        let code = i32::from_le_bytes(reply[..4].try_into().unwrap());
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Сервер вернул служебный ответ MTProto ({code})."),
        ));
    }
    validate_pq_reply(&reply, &nonce)
}

fn validate_pq_reply(reply: &[u8], nonce: &[u8; 16]) -> io::Result<()> {
    let malformed = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Неполный или неверный ответ resPQ.",
        )
    };
    if reply.len() < 56 || reply[..8] != [0; 8] || reply[8] & 1 == 0 {
        return Err(malformed());
    }
    let length = u32::from_le_bytes(reply[16..20].try_into().unwrap()) as usize;
    let payload = reply
        .get(20..)
        .and_then(|bytes| bytes.get(..length))
        .ok_or_else(malformed)?;
    // Plain MTProto handshakes can also carry protocol-level padding in a padded transport.
    // The pinned proxy's plain-packet decoder accepts up to 256 trailing bytes.
    if reply.len() - 20 - length > 256
        || payload.len() < 48
        || payload.len() % 4 != 0
        || payload[..4] != 0x05162463u32.to_le_bytes()
    {
        return Err(malformed());
    }
    if payload[4..20] != *nonce {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Ответ resPQ относится к другому запросу (nonce).",
        ));
    }
    // resPQ contains a server nonce, a big-endian pq no larger than 64 bits and a vector<long>.
    let pq_len = usize::from(payload[36]);
    if !(1..=8).contains(&pq_len) || payload[37..37 + pq_len].iter().all(|&b| b == 0) {
        return Err(malformed());
    }
    let vector = (37 + pq_len).next_multiple_of(4);
    let fields = payload
        .get(vector..)
        .filter(|p| p.len() >= 8)
        .ok_or_else(malformed)?;
    let count = u32::from_le_bytes(fields[4..8].try_into().unwrap()) as usize;
    if fields[..4] != 0x1cb5c415u32.to_le_bytes()
        || count == 0
        || count != (fields.len() - 8) / 8
        || (fields.len() - 8) % 8 != 0
    {
        return Err(malformed());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn hex<const N: usize>(text: &str) -> [u8; N] {
        assert_eq!(text.len(), N * 2);
        std::array::from_fn(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
    }
    #[test]
    fn opens_independent_aes_ctr_wire_vector() {
        // Produced with OpenSSL AES-256-CTR and SHA-256, retaining only the encrypted header tail.
        let header = hex(
            "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738800e374a25482755",
        );
        let key = std::array::from_fn(|i| i as u8);
        let mut session = open(header, &key, false).unwrap();
        assert_eq!(
            (session.dc, session.media, session.test, session.tag),
            (4, true, true, PADDED)
        );
        assert!(open(header, &[1; 16], false).is_err());
        let mut inbound = hex::<17>("f4f3e3ac40fa93716d32d046a0efbc8b66");
        session.client_in.apply_keystream(&mut inbound);
        assert_eq!(inbound, std::array::from_fn(|i| i as u8));
        session.client_out.apply_keystream(&mut inbound);
        assert_eq!(inbound, hex("61416a68f3cca870d67562f8a1e44019e2"));
        let (mut remote_in, mut remote_out) = ciphers(&session.init, None);
        let mut relay = session.init;
        remote_in.apply_keystream(&mut relay);
        assert_eq!(relay[56..60], PADDED);
        assert_eq!(relay[60..62], (-4i16).to_le_bytes());
        let mut part = *b"hello";
        session.relay_out.apply_keystream(&mut part);
        remote_in.apply_keystream(&mut part);
        assert_eq!(&part, b"hello");
        remote_out.apply_keystream(&mut part);
        session.relay_in.apply_keystream(&mut part);
        assert_eq!(&part, b"hello");
    }
    #[test]
    fn splits_fragmented_and_coalesced_packets() {
        let mut splitter = Splitter::new(ABRIDGED);
        assert!(splitter.split(&[1, 2]).is_empty());
        assert_eq!(
            splitter.split(&[3, 4, 5, 0xff, 1, 0, 0, 6, 7, 8, 9]),
            vec![vec![1, 2, 3, 4, 5], vec![0xff, 1, 0, 0, 6, 7, 8, 9]]
        );
        for tag in [INTERMEDIATE, PADDED] {
            let mut splitter = Splitter::new(tag);
            let bytes = [4, 0, 0, 128, 1, 2, 3, 4];
            assert!(splitter.split(&bytes[..3]).is_empty());
            assert_eq!(splitter.split(&bytes[3..]), vec![bytes]);
            assert!(splitter.flush().is_none());
            assert!(splitter.split(&bytes[..3]).is_empty());
            assert_eq!(splitter.flush().unwrap(), bytes[..3]);
        }
    }
    // Wire-format response from core.telegram.org/mtproto/samples-auth_key, including three fingerprints.
    fn sample_reply() -> [u8; 100] {
        hex(concat!(
            "000000000000000001f4ccc26170466a500000006324160551a1143fc7a3666b",
            "e4be54d6890a02dc63248f6748214eab8a2f4cc876e11974082e9cdb98c80cda",
            "4b00000015c4b51c0300000085fd64de851d9dd0a5b7f709355fc30b216be86c",
            "022bb4c3"
        ))
    }
    #[test]
    fn verifies_complete_tl_sample_nonce_and_plain_transport_padding() {
        let mut reply = sample_reply().to_vec();
        let nonce: [u8; 16] = reply[24..40].try_into().unwrap();
        validate_pq_reply(&reply, &nonce).unwrap();
        reply.extend_from_slice(&[0xa5; 256]);
        validate_pq_reply(&reply, &nonce).unwrap();
        assert!(validate_pq_reply(&reply, &[1; 16]).is_err());
        reply.push(0);
        assert!(validate_pq_reply(&reply, &nonce).is_err());
        let mut damaged = sample_reply();
        damaged[72..76].copy_from_slice(&4u32.to_le_bytes());
        assert!(validate_pq_reply(&damaged, &nonce).is_err());
        let mut truncated = sample_reply()[..56].to_vec();
        truncated[16..20].copy_from_slice(&36u32.to_le_bytes());
        assert!(validate_pq_reply(&truncated, &nonce).is_err());
    }

    #[test]
    fn handles_each_transport_for_media_and_test_datacenters() {
        let key = [0x65; 16];
        for tag in [ABRIDGED, INTERMEDIATE, PADDED] {
            for (requested, force, dc, media, test) in [
                (2, false, 2, false, false),
                (-4, false, 4, true, false),
                (10003, false, 3, false, true),
                (-10002, false, 2, true, true),
                (1, true, 1, false, true),
            ] {
                let (header, mut sender, mut recipient) =
                    generate_init(tag, requested, Some(&key)).unwrap();
                let mut session = open(header, &key, force).unwrap();
                assert_eq!(
                    (session.tag, session.dc, session.media, session.test),
                    (tag, dc, media, test)
                );
                let message = *b"typed transport";
                let mut wire = message;
                sender.apply_keystream(&mut wire);
                session.client_in.apply_keystream(&mut wire);
                assert_eq!(wire, message);
                session.client_out.apply_keystream(&mut wire);
                recipient.apply_keystream(&mut wire);
                assert_eq!(wire, message);
            }
        }
    }

    pub(crate) async fn respond_to_probe(
        server: impl AsyncRead + AsyncWrite + Unpin,
        secret: Option<&[u8; 16]>,
        correct_nonce: bool,
    ) {
        respond_to_probe_for_dc(server, secret, 2, correct_nonce).await;
    }

    pub(crate) async fn respond_to_probe_for_dc(
        mut server: impl AsyncRead + AsyncWrite + Unpin,
        secret: Option<&[u8; 16]>,
        expected_dc: i16,
        correct_nonce: bool,
    ) {
        let mut header = [0; 64];
        server.read_exact(&mut header).await.unwrap();
        let (mut incoming, mut outgoing) = ciphers(&header, secret);
        incoming.apply_keystream(&mut header);
        assert_eq!(header[56..60], PADDED);
        assert_eq!(header[60..62], expected_dc.to_le_bytes());
        let mut len = [0; 4];
        server.read_exact(&mut len).await.unwrap();
        incoming.apply_keystream(&mut len);
        let mut payload = vec![0; u32::from_le_bytes(len) as usize];
        assert!((40..=55).contains(&payload.len()));
        server.read_exact(&mut payload).await.unwrap();
        incoming.apply_keystream(&mut payload);
        assert_eq!(payload[..8], [0; 8]);
        assert_eq!(payload[16..20], 20_u32.to_le_bytes());
        assert_eq!(payload[20..24], 0xbe7e8ef1_u32.to_le_bytes());
        let mut packet = sample_reply().to_vec();
        packet[24..40].copy_from_slice(&payload[24..40]);
        if !correct_nonce {
            packet[24] ^= 0xff;
        }
        packet.extend_from_slice(&[0xa5; 64]);
        let mut reply = (packet.len() as u32).to_le_bytes().to_vec();
        reply.extend_from_slice(&packet);
        outgoing.apply_keystream(&mut reply);
        server.write_all(&reply).await.unwrap();
        server.flush().await.unwrap();
    }

    #[tokio::test]
    async fn probe_requires_an_answer_from_the_dc_to_its_own_request() {
        let secret = [0x26; 16];
        for correct_nonce in [true, false] {
            let (client, server) = tokio::io::duplex(1024);
            let respond = respond_to_probe(server, Some(&secret), correct_nonce);
            let (result, ()) = tokio::join!(probe(client, &secret, 2), respond);
            assert_eq!(result.is_ok(), correct_nonce);
        }
        let (client, server) = tokio::io::duplex(1024);
        drop(server);
        assert!(probe(client, &secret, 2).await.is_err());
    }
}
