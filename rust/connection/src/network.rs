use sha2::{Digest, Sha256};

// .NET hashes a semicolon-separated list of sorted, lowercase GUIDs ("D" format).
fn profile_key(mut ids: Vec<String>) -> Option<String> {
    ids.retain(|s| s != "00000000-0000-0000-0000-000000000000");
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return None;
    }
    Some(format!("{:x}", Sha256::digest(ids.join(";").as_bytes())))
}

#[cfg(windows)]
pub fn network_identity() -> Option<String> {
    use windows::Win32::{
        Networking::NetworkListManager::{
            INetworkListManager, NLM_ENUM_NETWORK_CONNECTED, NetworkListManager,
        },
        System::Com::{
            CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
        },
    };
    unsafe {
        // The worker owns this COM initialization; all network interfaces drop before uninitializing.
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
        let key = (|| {
            let manager: INetworkListManager =
                CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok()?;
            let networks = manager.GetNetworks(NLM_ENUM_NETWORK_CONNECTED).ok()?;
            let mut keys = Vec::new();
            loop {
                let mut item = [None];
                let mut fetched = 0u32;
                networks.Next(&mut item, Some(&mut fetched)).ok()?;
                if fetched == 0 {
                    break;
                }
                let id = item[0].as_ref()?.GetNetworkId().ok()?;
                let n = id.to_u128();
                let hex = format!("{n:032x}");
                keys.push(format!(
                    "{}-{}-{}-{}-{}",
                    &hex[..8],
                    &hex[8..12],
                    &hex[12..16],
                    &hex[16..20],
                    &hex[20..]
                ));
            }
            profile_key(keys)
        })();
        CoUninitialize();
        key
    }
}

#[cfg(not(windows))]
pub fn network_identity() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_is_order_independent_and_duplicates_do_not_change_it() {
        let a = "aaaaaaaa-abcd-4000-8000-000000000000".to_owned();
        let b = "bbbbbbbb-abcd-4000-8000-000000000000".to_owned();
        assert_eq!(
            profile_key(vec![b.clone(), a.clone(), a.clone()]),
            profile_key(vec![a, b])
        );
        assert_eq!(
            profile_key(vec!["00000000-0000-0000-0000-000000000000".into()]),
            None
        );
    }
}
