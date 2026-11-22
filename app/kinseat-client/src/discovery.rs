use std::{net::IpAddr, sync::mpsc::Sender, thread, time::Duration};

pub const SERVICE: &str = "_nvstream._tcp.local.";

/// Données de la demande d'accès liée à cette place. Le jeton ne doit pas être journalisé.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionAccess {
    pub request_id: String,
    pub token: String,
    pub address: String,
    pub port: u16,
}
impl std::fmt::Debug for SessionAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionAccess")
            .field("request_id", &self.request_id)
            .field("token", &"[masqué]")
            .field("address", &self.address)
            .field("port", &self.port)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Place {
    pub name: String,
    pub address: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_access: Option<SessionAccess>,
}
impl Place {
    pub fn host(&self) -> String {
        if self.address.contains(':') {
            format!("[{}]:{}", self.address, self.port)
        } else {
            format!("{}:{}", self.address, self.port)
        }
    }
}

/// IPv6 lien-local (fe80::/10).
pub fn is_link_local_v6(address: &str) -> bool {
    matches!(address.parse::<IpAddr>(), Ok(IpAddr::V6(v6)) if (v6.segments()[0] & 0xffc0) == 0xfe80)
}

/// Ajoute une annonce mDNS à la liste : un PC = une entrée. L'IPv4 est préférée à l'IPv6, et une
/// IPv6 lien-local (inutilisable sans identifiant de zone) n'est jamais retenue.
pub fn merge(places: &mut Vec<Place>, new: Place) {
    if is_link_local_v6(&new.address) {
        return;
    }
    let new_v4 = !new.address.contains(':');
    match places.iter_mut().find(|p| p.name == new.name) {
        Some(existing) => {
            if new_v4 && existing.address.contains(':') {
                *existing = new;
            }
        }
        None => places.push(new),
    }
}

/// Manual addresses must be IP literals (including bracketed IPv6) or DNS names, optionally with a port.
pub fn parse_address(raw: &str) -> Result<Place, &'static str> {
    let raw = raw.trim();
    let (address, port) = if let Some(rest) = raw.strip_prefix('[') {
        let (ip, suffix) = rest.split_once(']').ok_or("Adresse IPv6 invalide")?;
        ip.parse::<IpAddr>().map_err(|_| "Adresse IPv6 invalide")?;
        let port = suffix.strip_prefix(':').ok_or("Port manquant")?;
        (ip, port.parse::<u16>().map_err(|_| "Port invalide")?)
    } else if let Some((host, port)) = raw.rsplit_once(':') {
        (host, port.parse::<u16>().map_err(|_| "Port invalide")?)
    } else {
        (raw, 47989)
    };
    if port == 0
        || address.is_empty()
        || address.contains(['/', '\\', ' '])
        || (address.contains(':') && raw.parse::<std::net::SocketAddr>().is_err())
        || address.starts_with('-')
    {
        return Err("Adresse ou port invalide");
    }
    Ok(Place {
        name: address.to_owned(),
        address: address.to_owned(),
        port,
        session_access: None,
    })
}

pub fn browse(tx: Sender<Place>) {
    thread::spawn(move || {
        let Ok(daemon) = mdns_sd::ServiceDaemon::new() else {
            return;
        };
        let Ok(receiver) = daemon.browse(SERVICE) else {
            return;
        };
        loop {
            match receiver.recv_timeout(Duration::from_secs(1)) {
                Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                    for ip in info.get_addresses() {
                        let place = Place {
                            name: info
                                .get_fullname()
                                .trim_end_matches(SERVICE)
                                .trim_end_matches('.')
                                .to_owned(),
                            address: ip.to_string(),
                            port: info.get_port(),
                            session_access: None,
                        };
                        if tx.send(place).is_err() {
                            let _ = daemon.shutdown();
                            return;
                        }
                    }
                }
                Err(flume::RecvTimeoutError::Disconnected) => break,
                _ => {}
            }
        }
        let _ = daemon.shutdown();
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_place_files_and_redacted_debug() {
        let old = r#"{"name":"Salon","address":"127.0.0.1","port":48100}"#;
        let place: Place = serde_json::from_str(old).unwrap();
        assert!(place.session_access.is_none());
        let mut with_access = place;
        with_access.session_access = Some(SessionAccess {
            request_id: "request-123".into(),
            token: "token-0123456789abcdef".into(),
            address: "127.0.0.1".into(),
            port: 47960,
        });
        let saved = serde_json::to_string(&with_access).unwrap();
        assert_eq!(serde_json::from_str::<Place>(&saved).unwrap(), with_access);
        assert!(!format!("{with_access:?}").contains("token-0123456789abcdef"));
    }

    #[test]
    fn manual_address() {
        assert_eq!(
            parse_address("192.168.1.2:48100").unwrap().host(),
            "192.168.1.2:48100"
        );
        assert_eq!(parse_address("host.local").unwrap().port, 47989);
        assert_eq!(parse_address("[::1]:48100").unwrap().host(), "[::1]:48100");
        for bad in ["", "x:0", "x:bad", "[::1]", "host/evil:100", "-flag:200"] {
            assert!(parse_address(bad).is_err(), "{bad}");
        }
    }
    fn place(name: &str, address: &str) -> Place {
        Place {
            name: name.into(),
            address: address.into(),
            port: 47989,
            session_access: None,
        }
    }
    #[test]
    fn one_entry_per_pc_ipv4_preferred_no_link_local() {
        let mut places = Vec::new();
        merge(&mut places, place("SALON", "fe80::1"));
        assert!(places.is_empty());
        merge(&mut places, place("SALON", "2a01:cb00::5"));
        merge(&mut places, place("SALON", "192.168.1.20"));
        merge(&mut places, place("SALON", "2a01:cb00::6"));
        merge(&mut places, place("BUREAU", "192.168.1.30"));
        assert_eq!(
            places,
            vec![
                place("SALON", "192.168.1.20"),
                place("BUREAU", "192.168.1.30")
            ]
        );
        assert!(is_link_local_v6("fe80::abcd"));
        assert!(!is_link_local_v6("192.168.1.2"));
        assert!(!is_link_local_v6("2a01::1"));
    }
}
