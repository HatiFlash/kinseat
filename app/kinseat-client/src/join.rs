//! Demandes d'accès auprès de l'Hôte (contrat HTTP : docs/join-requests.md §1) : client HTTP/1.1
//! minimal sur `TcpStream`, analyse des réponses JSON, machine d'état du flux et sondes `hello`.
use crate::{
    discovery::{Place, SessionAccess},
    engine::{self, Engine, Event},
};
use rand::Rng;
use serde::Deserialize;
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

/// Port de l'écouteur LAN de l'Hôte.
pub const PORT: u16 = 47960;
const HELLO_TIMEOUT: Duration = Duration::from_millis(1500);
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// Le POST `/pin` attend la fin réelle de l'appairage Apollo côté Hôte
/// (`PairTimeout` = 45 s) : il doit dépasser ce délai, sinon le client abandonne
/// et la fermeture de la connexion annule l'appairage.
const PIN_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_EVERY: Duration = Duration::from_secs(2);
/// L'Hôte expire une demande après 10 min ; on abandonne un peu après.
const WAIT_LIMIT: Duration = Duration::from_secs(11 * 60);
const PAIR_LIMIT: Duration = Duration::from_secs(5 * 60);
const MAX_ERRORS: u8 = 5;
const MAX_RESPONSE: u64 = 64 * 1024;
const REPROBE_AFTER: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------- HTTP

fn decode_chunked(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = data.windows(2).position(|w| w == b"\r\n")?;
        let size_line = std::str::from_utf8(&data[..eol]).ok()?;
        let size = usize::from_str_radix(size_line.split(';').next()?.trim(), 16).ok()?;
        data = &data[eol + 2..];
        if size == 0 {
            return Some(out);
        }
        if data.len() < size + 2 {
            return None;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

/// Analyse une réponse HTTP/1.x complète : (code, corps).
pub fn parse_response(raw: &[u8]) -> Result<(u16, String), String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("Réponse HTTP incomplète")?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines.next().unwrap_or_default();
    let code = status
        .strip_prefix("HTTP/1.")
        .and_then(|s| s.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or("Réponse HTTP invalide")?;
    let chunked = lines.any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    let body = &raw[split + 4..];
    let body = if chunked {
        decode_chunked(body).ok_or("Corps HTTP invalide")?
    } else {
        body.to_vec()
    };
    Ok((code, String::from_utf8_lossy(&body).into_owned()))
}

pub fn request(
    address: &str,
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
    timeout: Duration,
) -> Result<(u16, String), String> {
    let host = if address.contains(':') {
        format!("[{address}]")
    } else {
        address.to_owned()
    };
    let target = format!("{host}:{port}");
    let addr = target
        .to_socket_addrs()
        .map_err(|_| "Adresse invalide".to_owned())?
        .next()
        .ok_or("Adresse introuvable")?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("Connexion impossible : {e}"))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\nAccept: application/json\r\n"
    );
    if let Some(token) = token {
        head.push_str(&format!("X-Kinseat-Token: {token}\r\n"));
    }
    let payload = body.unwrap_or("");
    if body.is_some() {
        head.push_str("Content-Type: application/json\r\n");
    }
    if method == "POST" {
        head.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    }
    head.push_str("\r\n");
    head.push_str(payload);
    stream
        .write_all(head.as_bytes())
        .map_err(|e| format!("Envoi impossible : {e}"))?;
    let mut raw = Vec::new();
    if let Err(e) = (&stream).take(MAX_RESPONSE).read_to_end(&mut raw) {
        if raw.is_empty() {
            return Err(format!("Lecture impossible : {e}"));
        }
    }
    parse_response(&raw)
}

// ------------------------------------------------------- JSON du contrat

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostInfo {
    pub name: String,
    pub address: String,
    pub version: String,
}

#[derive(Deserialize)]
struct Hello {
    #[serde(default)]
    kinseat: bool,
    #[serde(default)]
    host: String,
    #[serde(default)]
    version: String,
}

fn clean_text(raw: &str, max: usize) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

pub fn parse_hello(address: &str, body: &str) -> Result<HostInfo, String> {
    let hello: Hello = serde_json::from_str(body).map_err(|_| "Réponse hello invalide")?;
    let name = clean_text(&hello.host, 64);
    if !hello.kinseat || name.is_empty() {
        return Err("Ce n'est pas un PC Kinseat.".into());
    }
    Ok(HostInfo {
        name,
        address: address.to_owned(),
        version: clean_text(&hello.version, 16),
    })
}

pub fn hello(address: &str) -> Result<HostInfo, String> {
    let (code, body) = request(
        address,
        PORT,
        "GET",
        "/kinseat/v1/hello",
        None,
        None,
        HELLO_TIMEOUT,
    )?;
    if code != 200 {
        return Err(format!("Réponse inattendue ({code})."));
    }
    parse_hello(address, &body)
}

#[derive(PartialEq, Eq)]
pub struct Accepted {
    pub request_id: String,
    pub token: String,
}
impl std::fmt::Debug for Accepted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Accepted")
            .field("request_id", &self.request_id)
            .field("token", &"[masqué]")
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcceptedRaw {
    request_id: String,
    token: String,
}

/// L'identifiant et le jeton finissent dans une URL et un en-tête : on n'accepte que leurs jeux de
/// caractères prévus (GUID ; base64url).
pub fn parse_join_accepted(body: &str) -> Result<Accepted, String> {
    let raw: AcceptedRaw =
        serde_json::from_str(body).map_err(|_| "Réponse de demande invalide.".to_owned())?;
    let id_ok = valid_request_id(&raw.request_id);
    let token_ok = valid_token(&raw.token);
    if id_ok && token_ok {
        Ok(Accepted {
            request_id: raw.request_id,
            token: raw.token,
        })
    } else {
        Err("Réponse de demande invalide.".into())
    }
}

fn valid_request_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn valid_token(token: &str) -> bool {
    (16..=128).contains(&token.len())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '='))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionReply {
    closed_by_host: bool,
    closed_at_utc: Option<String>,
}

/// Seul un booléen explicite de l'Hôte confirme la fermeture.
pub fn parse_session_closed(body: &str) -> Option<bool> {
    let reply: SessionReply = serde_json::from_str(body).ok()?;
    let _ = reply.closed_at_utc;
    Some(reply.closed_by_host)
}

pub fn session_closed_by_host(access: &SessionAccess) -> Option<bool> {
    if !valid_request_id(&access.request_id)
        || !valid_token(&access.token)
        || access.port == 0
        || access.address.is_empty()
        || access.address.contains(['/', '\\', ' ', '\r', '\n'])
    {
        return None;
    }
    let path = format!("/kinseat/v1/join/{}/session", access.request_id);
    let (code, body) = request(
        &access.address,
        access.port,
        "GET",
        &path,
        Some(&access.token),
        None,
        Duration::from_millis(900),
    )
    .ok()?;
    (code == 200).then(|| parse_session_closed(&body)).flatten()
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct PlaceInfo {
    pub name: String,
    pub address: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct StatusReply {
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub place: Option<PlaceInfo>,
}

pub fn parse_status(body: &str) -> Result<StatusReply, String> {
    serde_json::from_str(body).map_err(|_| "Réponse d'état invalide.".to_owned())
}

/// Ce que l'état distant impose au client.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Keep,
    Refused,
    Expired,
    Accepted(Place),
    Paired(Option<Place>),
    Failed(String),
}

fn place_of(info: &PlaceInfo) -> Option<Place> {
    let address = info.address.trim();
    if address.is_empty() || info.port == 0 || address.contains(['/', '\\', ' ']) {
        return None;
    }
    Some(Place {
        name: clean_text(&info.name, 64),
        address: address.to_owned(),
        port: info.port,
        session_access: None,
    })
}

pub fn classify(reply: &StatusReply) -> Step {
    let place = reply.place.as_ref().and_then(place_of);
    match reply.status.as_str() {
        "pending" => Step::Keep,
        "refused" => Step::Refused,
        "expired" => Step::Expired,
        "accepted" => match place {
            Some(p) => Step::Accepted(p),
            None => Step::Failed("Le PC n'a pas indiqué la place attribuée.".into()),
        },
        "paired" => Step::Paired(place),
        "failed" => Step::Failed(
            reply
                .message
                .as_deref()
                .map(|m| clean_text(m, 200))
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "L'association a échoué sur le PC hôte.".into()),
        ),
        _ => Step::Failed("État de demande inconnu : mettez Kinseat à jour.".into()),
    }
}

// ----------------------------------------------------------- Machine d'état

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Sending,
    Waiting,
    Pairing,
    Ready(Place),
    Refused,
    Expired,
    Failed(String),
    Cancelled,
}
impl Phase {
    pub fn is_active(&self) -> bool {
        matches!(self, Phase::Sending | Phase::Waiting | Phase::Pairing)
    }
}

/// Résultats cumulés de l'appairage : la place n'est « prête » qu'avec `paired` côté Hôte ET le
/// succès de Moonlight.
#[derive(Default)]
pub struct Tracker {
    pub place: Option<Place>,
    pub host_paired: bool,
    pub moonlight: Option<Result<(), String>>,
}
impl Tracker {
    pub fn outcome(&self) -> Option<Phase> {
        match (&self.moonlight, self.host_paired, &self.place) {
            (Some(Err(e)), _, _) => Some(Phase::Failed(e.clone())),
            (Some(Ok(())), true, Some(place)) => Some(Phase::Ready(place.clone())),
            _ => None,
        }
    }
}

pub struct JoinParams {
    pub pseudo: String,
    pub device: String,
    pub device_id: String,
}

fn sleep_unless_cancelled(total: Duration, cancel: &AtomicBool) -> bool {
    let end = Instant::now() + total;
    while Instant::now() < end {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    cancel.load(Ordering::Relaxed)
}

fn http_failure(code: u16, body: &str) -> String {
    match code {
        429 => "Ce PC a trop de demandes en attente. Réessayez dans quelques minutes.".into(),
        400 | 422 => "Le PC a rejeté la demande (informations invalides).".into(),
        404 => "Le PC ne reconnaît plus cette demande.".into(),
        _ => parse_status(body)
            .ok()
            .and_then(|r| r.message)
            .map(|m| clean_text(&m, 200))
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| format!("Réponse inattendue du PC ({code}).")),
    }
}

fn drive(
    host: &HostInfo,
    params: &JoinParams,
    engine: Engine,
    tx: &Sender<Phase>,
    cancel: &AtomicBool,
) -> Phase {
    let send = |p: Phase| {
        let _ = tx.send(p);
    };
    send(Phase::Sending);
    let body = serde_json::json!({
        "pseudo": params.pseudo,
        "device": params.device,
        "deviceId": params.device_id,
    })
    .to_string();
    let accepted = match request(
        &host.address,
        PORT,
        "POST",
        "/kinseat/v1/join",
        None,
        Some(&body),
        CALL_TIMEOUT,
    ) {
        Ok((200 | 202, text)) => match parse_join_accepted(&text) {
            Ok(a) => a,
            Err(e) => return Phase::Failed(e),
        },
        Ok((code, text)) => return Phase::Failed(http_failure(code, &text)),
        Err(e) => return Phase::Failed(e),
    };
    send(Phase::Waiting);
    let base = format!("/kinseat/v1/join/{}", accepted.request_id);
    let token = accepted.token.as_str();
    let started = Instant::now();
    let mut pair_started: Option<Instant> = None;
    let mut engine = Some(engine);
    let mut pair_rx: Option<Receiver<Event>> = None;
    let engine_cancel = Arc::new(AtomicBool::new(false));
    let mut tracker = Tracker::default();
    let mut errors = 0u8;
    let finish = |phase: Phase| {
        engine_cancel.store(true, Ordering::Relaxed);
        phase
    };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return finish(Phase::Cancelled);
        }
        if pair_started.is_none() && started.elapsed() > WAIT_LIMIT {
            return finish(Phase::Expired);
        }
        if pair_started.is_some_and(|t| t.elapsed() > PAIR_LIMIT) {
            return finish(Phase::Failed(
                "Le PC n'a pas confirmé l'association à temps.".into(),
            ));
        }
        match request(
            &host.address,
            PORT,
            "GET",
            &base,
            Some(token),
            None,
            CALL_TIMEOUT,
        ) {
            Ok((200, text)) => {
                errors = 0;
                match parse_status(&text).map(|r| classify(&r)) {
                    Err(e) => return finish(Phase::Failed(e)),
                    Ok(Step::Keep) => {}
                    Ok(Step::Refused) => return finish(Phase::Refused),
                    Ok(Step::Expired) => return finish(Phase::Expired),
                    Ok(Step::Failed(m)) => return finish(Phase::Failed(m)),
                    Ok(Step::Paired(place)) => {
                        tracker.host_paired = true;
                        if tracker.place.is_none() {
                            tracker.place = place;
                        }
                    }
                    Ok(Step::Accepted(place)) => {
                        if let Some(engine) = engine.take() {
                            tracker.place = Some(place.clone());
                            let pin = format!("{:04}", rand::thread_rng().gen_range(0..10000));
                            send(Phase::Pairing);
                            pair_started = Some(Instant::now());
                            let (ptx, prx) = channel();
                            pair_rx = Some(prx);
                            engine::pair(engine, place, pin.clone(), ptx, engine_cancel.clone());
                            // Moonlight doit attendre le code avant que l'Hôte ne le transmette.
                            if sleep_unless_cancelled(Duration::from_millis(1500), cancel) {
                                return finish(Phase::Cancelled);
                            }
                            let pin_body = serde_json::json!({ "pin": pin }).to_string();
                            let path = format!("{base}/pin");
                            match request(
                                &host.address,
                                PORT,
                                "POST",
                                &path,
                                Some(token),
                                Some(&pin_body),
                                PIN_TIMEOUT,
                            ) {
                                Ok((200..=204, _)) => {}
                                Ok((code, text)) => {
                                    return finish(Phase::Failed(http_failure(code, &text)))
                                }
                                Err(e) => return finish(Phase::Failed(e)),
                            }
                        }
                    }
                }
            }
            Ok((404, _)) => return finish(Phase::Expired),
            Ok((code, text)) => {
                errors += 1;
                if errors >= MAX_ERRORS {
                    return finish(Phase::Failed(http_failure(code, &text)));
                }
            }
            Err(e) => {
                errors += 1;
                if errors >= MAX_ERRORS {
                    return finish(Phase::Failed(format!("Le PC ne répond plus ({e}).")));
                }
            }
        }
        if let Some(rx) = &pair_rx {
            while let Ok(event) = rx.try_recv() {
                if let Event::Pair(result) = event {
                    tracker.moonlight = Some(result);
                }
            }
        }
        if let Some(phase) = tracker.outcome() {
            return match phase {
                Phase::Ready(mut place) => {
                    place.session_access = Some(SessionAccess {
                        request_id: accepted.request_id.clone(),
                        token: accepted.token.clone(),
                        address: host.address.clone(),
                        port: PORT,
                    });
                    Phase::Ready(place)
                }
                other => other,
            };
        }
        if sleep_unless_cancelled(POLL_EVERY, cancel) {
            return finish(Phase::Cancelled);
        }
    }
}

// ------------------------------------------------------------ État de l'UI

pub struct ProbeResult {
    pub address: String,
    pub manual: bool,
    pub result: Result<HostInfo, String>,
}

pub struct JoinUi {
    pub hosts: Vec<HostInfo>,
    probing: HashMap<String, Instant>,
    probe_tx: Sender<ProbeResult>,
    probe_rx: Receiver<ProbeResult>,
    pub manual_pending: bool,
    pub phase: Option<Phase>,
    pub host: Option<HostInfo>,
    rx: Option<Receiver<Phase>>,
    cancel: Option<Arc<AtomicBool>>,
}
impl Default for JoinUi {
    fn default() -> Self {
        let (probe_tx, probe_rx) = channel();
        Self {
            hosts: Vec::new(),
            probing: HashMap::new(),
            probe_tx,
            probe_rx,
            manual_pending: false,
            phase: None,
            host: None,
            rx: None,
            cancel: None,
        }
    }
}
impl JoinUi {
    pub fn is_active(&self) -> bool {
        self.phase.as_ref().is_some_and(Phase::is_active)
    }
    fn probe(&mut self, address: String, manual: bool) {
        self.probing.insert(address.clone(), Instant::now());
        let tx = self.probe_tx.clone();
        thread::spawn(move || {
            let result = hello(&address);
            let _ = tx.send(ProbeResult {
                address,
                manual,
                result,
            });
        });
    }
    /// Interroge `hello` pour chaque PC découvert pas encore connu (dédoublonné par adresse).
    pub fn schedule(&mut self, places: &[Place]) {
        for place in places {
            let known = self.hosts.iter().any(|h| h.address == place.address);
            let recent = self
                .probing
                .get(&place.address)
                .is_some_and(|t| t.elapsed() < REPROBE_AFTER);
            if !known && !recent {
                self.probe(place.address.clone(), false);
            }
        }
    }
    pub fn probe_manual(&mut self, address: String) {
        self.manual_pending = true;
        self.probe(address, true);
    }
    /// Intègre les réponses des sondes ; renvoie un message pour une adresse saisie sans succès.
    pub fn poll_probes(&mut self) -> Option<String> {
        let mut notice = None;
        while let Ok(done) = self.probe_rx.try_recv() {
            if done.manual {
                self.manual_pending = false;
            }
            match done.result {
                Ok(info) => {
                    if !self.hosts.iter().any(|h| h.address == info.address) {
                        self.hosts.push(info);
                    }
                }
                Err(_) if done.manual => {
                    notice = Some(format!(
                        "Aucun PC Kinseat ne répond à l'adresse {}.",
                        done.address
                    ));
                }
                Err(_) => {}
            }
        }
        notice
    }
    pub fn start(&mut self, host: HostInfo, params: JoinParams, engine: Engine) {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.rx = Some(rx);
        self.cancel = Some(cancel.clone());
        self.phase = Some(Phase::Sending);
        self.host = Some(host.clone());
        thread::spawn(move || {
            let last = drive(&host, &params, engine, &tx, &cancel);
            let _ = tx.send(last);
        });
    }
    pub fn cancel(&mut self) {
        if let Some(flag) = self.cancel.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.rx = None;
        self.phase = None;
    }
    /// Applique un changement de phase ; renvoie la place quand la demande aboutit. Une phase
    /// reçue après un état final est ignorée.
    pub fn apply(&mut self, phase: Phase) -> Option<Place> {
        if self.phase.as_ref().is_some_and(|p| !p.is_active()) {
            return None;
        }
        let ready = match &phase {
            Phase::Ready(place) => Some(place.clone()),
            _ => None,
        };
        if !phase.is_active() {
            self.rx = None;
            self.cancel = None;
        }
        self.phase = Some(phase);
        ready
    }
    /// Reçoit les phases envoyées par le fil de travail.
    pub fn poll_phases(&mut self) -> Option<Place> {
        let mut ready = None;
        let mut phases = Vec::new();
        if let Some(rx) = &self.rx {
            while let Ok(p) = rx.try_recv() {
                phases.push(p);
            }
        }
        for p in phases {
            if let Some(place) = self.apply(p) {
                ready = Some(place);
            }
        }
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn pin_timeout_exceeds_host_pair_timeout_and_call_timeout() {
        assert!(PIN_TIMEOUT > Duration::from_secs(45));
        assert!(PIN_TIMEOUT > CALL_TIMEOUT);
    }

    fn place() -> Place {
        Place {
            name: "Salon 1".into(),
            address: "192.168.1.20".into(),
            port: 48100,
            session_access: None,
        }
    }

    /// Vrai flux client contre un vrai Hôte : `KINSEAT_LIVE_HOST=ip [KINSEAT_LIVE_PSEUDO=Lea]
    /// [KINSEAT_LIVE_DEVICE=PC-LABO] [KINSEAT_LIVE_DEVICE_ID=labtest-0001]
    /// [KINSEAT_LIVE_EXPECT=refused|paired] cargo test -p kinseat-client -- --ignored
    /// --nocapture live_join`
    #[test]
    #[ignore]
    fn live_join() {
        let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
        let address = std::env::var("KINSEAT_LIVE_HOST").expect("KINSEAT_LIVE_HOST");
        let expect = var("KINSEAT_LIVE_EXPECT", "paired");
        let params = JoinParams {
            pseudo: var("KINSEAT_LIVE_PSEUDO", "Lea"),
            device: var("KINSEAT_LIVE_DEVICE", "PC-LABO"),
            device_id: var("KINSEAT_LIVE_DEVICE_ID", "labtest-0001"),
        };
        let host = hello(&address).expect("hello");
        eprintln!("live_join hello: {host:?}");
        let engine = Engine::locate("").expect("moonlight on PATH");
        let (tx, rx) = channel();
        let reader = thread::spawn(move || {
            while let Ok(p) = rx.recv() {
                eprintln!("live_join phase: {p:?}");
            }
        });
        let cancel = AtomicBool::new(false);
        let last = drive(&host, &params, engine, &tx, &cancel);
        eprintln!("live_join final: {last:?}");
        drop(tx);
        let _ = reader.join();
        match expect.as_str() {
            "refused" => assert_eq!(last, Phase::Refused),
            "paired" => assert!(matches!(last, Phase::Ready(_)), "{last:?}"),
            other => panic!("KINSEAT_LIVE_EXPECT inconnu : {other}"),
        }
    }

    #[test]
    fn hello_parsing() {
        let ok = parse_hello(
            "10.0.0.2",
            r#"{"kinseat":true,"host":"SALON","version":"0.3.8"}"#,
        )
        .unwrap();
        assert_eq!(ok.name, "SALON");
        assert_eq!(ok.address, "10.0.0.2");
        assert_eq!(ok.version, "0.3.8");
        for bad in [
            r#"{"kinseat":false,"host":"X"}"#,
            r#"{"host":"X"}"#,
            r#"{"kinseat":true,"host":"  "}"#,
            "not json",
            "",
        ] {
            assert!(parse_hello("a", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn session_response_parsing() {
        assert_eq!(
            parse_session_closed(r#"{"closedByHost":true,"closedAtUtc":"2026-11-20T12:00:00Z"}"#),
            Some(true)
        );
        assert_eq!(
            parse_session_closed(r#"{"closedByHost":false,"closedAtUtc":null}"#),
            Some(false)
        );
        for invalid in ["{}", "not json", r#"{"closedByHost":"true"}"#] {
            assert_eq!(parse_session_closed(invalid), None);
        }
    }

    #[test]
    fn session_request_auth_and_path() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(req.starts_with("GET /kinseat/v1/join/request-123/session HTTP/1.1\r\n"));
            assert!(req.contains("X-Kinseat-Token: token-0123456789abcdef\r\n"));
            s.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"closedByHost\":true,\"closedAtUtc\":null}").unwrap();
        });
        let access = SessionAccess {
            request_id: "request-123".into(),
            token: "token-0123456789abcdef".into(),
            address: "127.0.0.1".into(),
            port,
        };
        assert_eq!(session_closed_by_host(&access), Some(true));
        server.join().unwrap();
        assert!(!format!("{access:?}").contains(&access.token));
        let mut invalid = access;
        invalid.token = "bad\r\nInjected: yes".into();
        assert_eq!(session_closed_by_host(&invalid), None);
    }

    #[test]
    fn join_response_parsing() {
        let body = r#"{"requestId":"3f2b8c1e-aaaa-bbbb-cccc-1234567890ab","token":"abcDEF012345_-abcDEF012345_-abcDEF0123"}"#;
        let a = parse_join_accepted(body).unwrap();
        assert_eq!(a.request_id, "3f2b8c1e-aaaa-bbbb-cccc-1234567890ab");
        assert!(
            parse_join_accepted(r#"{"requestId":"../x","token":"abcDEF012345_-abcDEF"}"#).is_err()
        );
        assert!(parse_join_accepted(
            r#"{"requestId":"3f2b8c1e-aaaa","token":"a b\r\nX: y1234567890"}"#
        )
        .is_err());
        assert!(parse_join_accepted("{}").is_err());
    }

    #[test]
    fn status_parsing_and_classification() {
        let r = parse_status(r#"{"status":"pending"}"#).unwrap();
        assert_eq!(classify(&r), Step::Keep);
        let r = parse_status(r#"{"status":"refused","message":"non"}"#).unwrap();
        assert_eq!(classify(&r), Step::Refused);
        let r = parse_status(r#"{"status":"expired"}"#).unwrap();
        assert_eq!(classify(&r), Step::Expired);
        let r = parse_status(
            r#"{"status":"accepted","place":{"name":"Salon 1","address":"192.168.1.20","port":48100}}"#,
        )
        .unwrap();
        assert_eq!(classify(&r), Step::Accepted(place()));
        let r = parse_status(r#"{"status":"accepted"}"#).unwrap();
        assert!(matches!(classify(&r), Step::Failed(_)));
        let r = parse_status(
            r#"{"status":"paired","place":{"name":"Salon 1","address":"192.168.1.20","port":48100}}"#,
        )
        .unwrap();
        assert_eq!(classify(&r), Step::Paired(Some(place())));
        let r = parse_status(r#"{"status":"failed","message":"Code refusé"}"#).unwrap();
        assert_eq!(classify(&r), Step::Failed("Code refusé".into()));
        let r = parse_status(r#"{"status":"failed"}"#).unwrap();
        assert!(matches!(classify(&r), Step::Failed(m) if !m.is_empty()));
        let r = parse_status(r#"{"status":"bizarre"}"#).unwrap();
        assert!(matches!(classify(&r), Step::Failed(_)));
        assert!(parse_status("x").is_err());
    }

    #[test]
    fn tracker_needs_both_confirmations() {
        let mut t = Tracker::default();
        assert_eq!(t.outcome(), None);
        t.place = Some(place());
        t.host_paired = true;
        assert_eq!(t.outcome(), None, "Moonlight pas encore confirmé");
        t.moonlight = Some(Ok(()));
        assert_eq!(t.outcome(), Some(Phase::Ready(place())));
        t.host_paired = false;
        assert_eq!(t.outcome(), None, "l'Hôte n'a pas confirmé");
        t.moonlight = Some(Err("Code incorrect".into()));
        assert_eq!(t.outcome(), Some(Phase::Failed("Code incorrect".into())));
    }

    #[test]
    fn ui_transitions() {
        let mut ui = JoinUi::default();
        assert!(!ui.is_active());
        ui.phase = Some(Phase::Sending);
        assert!(ui.is_active());
        assert_eq!(ui.apply(Phase::Waiting), None);
        assert_eq!(ui.apply(Phase::Pairing), None);
        assert!(ui.is_active());
        assert_eq!(ui.apply(Phase::Ready(place())), Some(place()));
        assert!(!ui.is_active());
        // Après un état final, plus rien ne change.
        assert_eq!(ui.apply(Phase::Waiting), None);
        assert_eq!(ui.phase, Some(Phase::Ready(place())));

        let mut ui = JoinUi {
            phase: Some(Phase::Waiting),
            ..JoinUi::default()
        };
        ui.apply(Phase::Refused);
        assert_eq!(ui.phase, Some(Phase::Refused));
        ui.cancel();
        assert_eq!(ui.phase, None);
    }

    #[test]
    fn response_parsing() {
        let plain = b"HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\n\r\n{\"a\":1}";
        assert_eq!(parse_response(plain).unwrap(), (202, "{\"a\":1}".into()));
        let chunked =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        assert_eq!(parse_response(chunked).unwrap(), (200, "{\"a\":1}".into()));
        assert!(parse_response(b"garbage").is_err());
        assert!(parse_response(b"HTTP/1.1 abc\r\n\r\n").is_err());
    }

    #[test]
    fn real_socket_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let body = r#"{"kinseat":true,"host":"TEST","version":"0.3.8"}"#;
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            req
        });
        let (code, body) = request(
            "127.0.0.1",
            port,
            "GET",
            "/kinseat/v1/hello",
            Some("tok"),
            None,
            Duration::from_secs(2),
        )
        .unwrap();
        let req = server.join().unwrap();
        assert!(req.starts_with("GET /kinseat/v1/hello HTTP/1.1\r\n"));
        assert!(req.contains("X-Kinseat-Token: tok\r\n"));
        assert_eq!(code, 200);
        assert_eq!(parse_hello("127.0.0.1", &body).unwrap().name, "TEST");
    }

    #[test]
    fn closed_port_fails_fast() {
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        assert!(request(
            "127.0.0.1",
            port,
            "GET",
            "/",
            None,
            None,
            Duration::from_millis(500)
        )
        .is_err());
    }
}
