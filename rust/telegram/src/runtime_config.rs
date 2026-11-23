use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io,
    net::{IpAddr, Ipv4Addr},
};

#[derive(Clone)]
pub(crate) struct Config {
    pub port: u16,
    pub secret: [u8; 16],
    pub dc_redirects: HashMap<u16, IpAddr>,
    pub buffer_size: usize,
    pub pool_size: usize,
    pub cfproxy: bool,
    pub h2: bool,
    pub no_secure: bool,
    pub force_test_dc: bool,
    pub cfproxy_user_domains: Vec<String>,
    pub cfproxy_worker_domains: Vec<String>,
    pub fake_tls_domain: Option<String>,
    pub proxy_protocol: bool,
}
fn bad(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Неверный параметр Telegram: {name}."),
    )
}
fn domains(value: Option<&Value>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut pieces: Vec<&str> = Vec::new();
    match value {
        Some(Value::String(s)) => pieces.push(s),
        Some(Value::Array(items)) => pieces.extend(items.iter().filter_map(Value::as_str)),
        _ => (),
    }
    pieces
        .into_iter()
        .flat_map(|s| s.split(|c: char| c.is_whitespace() || c == ',' || c == ';'))
        .filter(|s| !s.is_empty() && seen.insert(s.to_ascii_lowercase()))
        .map(str::to_owned)
        .collect()
}
impl Config {
    pub fn parse(raw: &str) -> io::Result<Self> {
        let value: Value = serde_json::from_str(raw)?;
        let cfg = value.as_object().ok_or_else(|| bad("config.json"))?;
        let flag = |name: &str, default: bool| {
            cfg.get(name)
                .map(|v| v.as_bool().ok_or_else(|| bad(name)))
                .unwrap_or(Ok(default))
        };
        let number = |name: &str, default: u64, max: u64| {
            cfg.get(name)
                .map(|v| v.as_u64().filter(|n| *n <= max).ok_or_else(|| bad(name)))
                .unwrap_or(Ok(default))
        };
        let port = u16::try_from(number("port", 1443, 65535)?).unwrap();
        if port == 0 {
            return Err(bad("port"));
        }
        let encoded = cfg
            .get("secret")
            .and_then(|v| v.as_str())
            .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| bad("secret"))?;
        let secret =
            std::array::from_fn(|i| u8::from_str_radix(&encoded[i * 2..i * 2 + 2], 16).unwrap());
        let mut dc_redirects = HashMap::new();
        if let Some(v) = cfg.get("dc_ip") {
            for entry in v.as_array().ok_or_else(|| bad("dc_ip"))? {
                let (dc, ip) = entry
                    .as_str()
                    .and_then(|s| s.split_once(':'))
                    .ok_or_else(|| bad("dc_ip"))?;
                dc_redirects.insert(
                    dc.parse().map_err(|_| bad("dc_ip"))?,
                    IpAddr::V4(ip.parse::<Ipv4Addr>().map_err(|_| bad("dc_ip"))?),
                );
            }
        } else {
            let redirect = IpAddr::V4(Ipv4Addr::new(149, 154, 167, 220));
            dc_redirects.insert(2, redirect);
            dc_redirects.insert(4, redirect);
        }
        let pool = |field: &str, enabled: &str| -> io::Result<Vec<String>> {
            let names = domains(cfg.get(field));
            Ok(if flag(enabled, !names.is_empty())? {
                names
            } else {
                vec![]
            })
        };
        let domain = cfg
            .get("fake_tls_domain")
            .map(|v| v.as_str().ok_or_else(|| bad("fake_tls_domain")))
            .transpose()?
            .filter(|s| !s.is_empty());
        if let Some(host) = domain
            && (host.len() > 253
                || !host
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-')))
        {
            return Err(bad("fake_tls_domain"));
        }
        Ok(Self {
            port,
            secret,
            dc_redirects,
            buffer_size: number("buf_kb", 256, 4096)?.max(4) as usize * 1024,
            pool_size: number("pool_size", 4, 64)? as usize,
            cfproxy: flag("cfproxy", true)?,
            h2: flag("h2", true)?,
            no_secure: flag("no_secure", false)?,
            force_test_dc: flag("force_test_dc", false)?,
            cfproxy_user_domains: pool("cfproxy_user_domain", "cfproxy_user_domain_enabled")?,
            cfproxy_worker_domains: pool("cfproxy_worker_domain", "cfproxy_worker_enabled")?,
            fake_tls_domain: domain.map(str::to_owned),
            proxy_protocol: flag("proxy_protocol", false)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_defaults_and_advanced_overrides() {
        let mut v =
            serde_json::json!({"host":"0.0.0.0","secret":"ABCDEF0123456789abcdef0123456789"});
        let c = Config::parse(&v.to_string()).unwrap();
        assert_eq!(c.port, 1443);
        assert_eq!(c.dc_redirects.len(), 2);
        assert_eq!(c.buffer_size, 262144);
        assert!(c.h2 && c.cfproxy);
        assert!(c.fake_tls_domain.is_none() && !c.proxy_protocol);
        v["dc_ip"] = serde_json::json!(["2:127.0.0.1"]);
        v["cfproxy_worker_domain"] = serde_json::json!(["a.local,b.local", "A.local"]);
        v["cfproxy_user_domain"] = serde_json::json!("u.local");
        v["cfproxy_user_domain_enabled"] = false.into();
        let c = Config::parse(&v.to_string()).unwrap();
        assert_eq!(c.dc_redirects[&2], Ipv4Addr::LOCALHOST);
        assert_eq!(c.cfproxy_worker_domains, vec!["a.local", "b.local"]);
        assert!(c.cfproxy_user_domains.is_empty());
        v["pool_size"] = true.into();
        assert!(Config::parse(&v.to_string()).is_err());
    }
}
