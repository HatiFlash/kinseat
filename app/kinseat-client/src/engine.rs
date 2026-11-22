use crate::{discovery::Place, settings::Settings};
use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct Engine {
    pub program: PathBuf,
    pub prefix: Vec<String>,
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}
impl Engine {
    pub fn locate(custom: &str) -> Option<Self> {
        if !custom.trim().is_empty() {
            let p = PathBuf::from(custom.trim());
            return p.is_file().then_some(Self {
                program: p,
                prefix: vec![],
            });
        }
        #[cfg(windows)]
        {
            if let Some(path) = std::env::current_exe()
                .ok()
                .and_then(|p| {
                    p.parent()
                        .map(|p| p.join("moonlight").join("Moonlight.exe"))
                })
                .filter(|p| p.is_file())
            {
                return Some(Self {
                    program: path,
                    prefix: vec![],
                });
            }
            on_path("Moonlight.exe").map(|program| Self {
                program,
                prefix: vec![],
            })
        }
        #[cfg(not(windows))]
        {
            on_path("moonlight")
                .or_else(|| on_path("Moonlight"))
                .map(|program| Self {
                    program,
                    prefix: vec![],
                })
                .or_else(|| {
                    on_path("flatpak").map(|program| Self {
                        program,
                        prefix: vec!["run".into(), "com.moonlight_stream.Moonlight".into()],
                    })
                })
        }
    }
    pub fn command(&self, args: &[String]) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix).args(args);
        // The bundled Windows build is portable (`portable.dat`): it keeps its settings, including
        // the pairing certificates, in its *working directory*. Pin that to a per-user writable
        // folder, never Program Files or wherever the client happened to be started from.
        // Moonlight only turns portable mode on when `portable.dat` sits in that directory, so
        // carry the marker over from the bundled copy.
        if let Some(dir) = data_dir() {
            if fs::create_dir_all(&dir).is_ok() {
                let bundled_marker = self.program.parent().map(|p| p.join("portable.dat"));
                if let Some(marker) = bundled_marker.filter(|m| m.is_file()) {
                    let target = dir.join("portable.dat");
                    if !target.exists() {
                        let _ = fs::copy(marker, target);
                    }
                }
                cmd.current_dir(dir);
            }
        }
        cmd
    }
}

/// Journal minimal du raccourci de flux ; une erreur d'E/S ne doit jamais bloquer le crochet.
#[cfg(windows)]
pub fn diagnostic(msg: &str) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let Some(path) = crate::settings::config_path()
        .and_then(|p| p.parent().map(|dir| dir.join("logs").join("client.log")))
    else {
        return;
    };
    let Some(dir) = path.parent() else {
        return;
    };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(mut log) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
        let _ = writeln!(log, "{}.{:03} {msg}", now.as_secs(), now.subsec_millis());
    }
}

/// Per-user working directory of the streaming engine.
pub fn data_dir() -> Option<PathBuf> {
    crate::settings::config_path().and_then(|p| p.parent().map(|p| p.join("moonlight")))
}

const MOONLIGHT_ORG: &str = "Moonlight Game Streaming Project";

/// Moonlight settings files that may hold its paired hosts.
fn settings_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(dir) = data_dir() {
        files.push(dir.join(MOONLIGHT_ORG).join("Moonlight.ini"));
    }
    #[cfg(not(windows))]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        if let Some(config) = config {
            files.push(config.join(MOONLIGHT_ORG).join("Moonlight.conf"));
        }
        if let Some(home) = std::env::var_os("HOME") {
            files.push(
                PathBuf::from(home)
                    .join(".var/app/com.moonlight_stream.Moonlight/config")
                    .join(MOONLIGHT_ORG)
                    .join("Moonlight.conf"),
            );
        }
    }
    files
}

fn is_real_pem_cert(value: &str) -> bool {
    let decoded = value.replace("\\n", "\n");
    let Some((_, rest)) = decoded.split_once("-----BEGIN CERTIFICATE-----") else {
        return false;
    };
    let Some((body, _)) = rest.split_once("-----END CERTIFICATE-----") else {
        return false;
    };
    !body.chars().all(char::is_whitespace)
}

/// Registry values may be `REG_SZ` text or `REG_BINARY` hex: reject only what is clearly empty,
/// i.e. `@ByteArray()` or a PEM header/footer with no body (Moonlight's mid-pairing placeholder).
#[cfg_attr(not(windows), allow(dead_code))]
fn registry_cert_is_usable(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value == "@ByteArray()" {
        return false;
    }
    let text = if value.chars().all(|c| c.is_ascii_hexdigit()) && value.len() % 2 == 0 {
        (0..value.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
            .map(char::from)
            .collect::<String>()
    } else {
        value.to_owned()
    };
    if text.contains("-----BEGIN CERTIFICATE-----") {
        return is_real_pem_cert(&text);
    }
    true
}

/// Server certificate Moonlight stored for the host with this `uuid`, from QSettings INI text.
pub fn srvcert_in_ini(text: &str, uuid: &str) -> Option<String> {
    let mut in_hosts = false;
    let mut uuids = std::collections::HashMap::new();
    let mut certs = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_hosts = line == "[hosts]";
            continue;
        }
        if !in_hosts {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some((index, name)) = key.split_once('\\') else {
            continue;
        };
        match name {
            "uuid" => {
                uuids.insert(index.to_owned(), value.trim().to_owned());
            }
            "srvcert" if is_real_pem_cert(value.trim()) => {
                certs.insert(index.to_owned(), value.trim().to_owned());
            }
            _ => {}
        }
    }
    uuids
        .into_iter()
        .find(|(_, u)| u.eq_ignore_ascii_case(uuid))
        .and_then(|(index, _)| certs.remove(&index))
}

/// Removes the stored server certificate of host `uuid` from QSettings INI text, so that a new
/// pairing with the same host (same certificate) can be told apart from a stale one.
pub fn without_srvcert(text: &str, uuid: &str) -> Option<String> {
    let mut index = None;
    let mut in_hosts = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_hosts = t == "[hosts]";
        } else if in_hosts {
            if let Some((key, value)) = t.split_once('=') {
                if let Some((i, "uuid")) = key.split_once('\\') {
                    if value.trim().eq_ignore_ascii_case(uuid) {
                        index = Some(i.to_owned());
                    }
                }
            }
        }
    }
    let target = format!("{}\\srvcert=", index?);
    let mut in_hosts = false;
    let mut removed = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with('[') {
            in_hosts = t == "[hosts]";
        } else if in_hosts && t.starts_with(&target) {
            removed = true;
            continue;
        }
        out.push_str(line);
    }
    removed.then_some(out)
}

/// Server certificate stored in the registry by an installed (non-portable) Moonlight.
#[cfg(windows)]
fn srvcert_in_registry(uuid: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let out = Command::new("reg.exe")
        .args([
            "query",
            &format!("HKCU\\Software\\{MOONLIGHT_ORG}\\Moonlight\\hosts"),
            "/s",
        ])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut current_uuid, mut current_cert): (Option<String>, Option<String>) = (None, None);
    let mut found = None;
    let mut flush = |u: &mut Option<String>, c: &mut Option<String>| {
        if u.as_deref().is_some_and(|u| u.eq_ignore_ascii_case(uuid)) {
            found = c.take();
        }
        *u = None;
        *c = None;
    };
    for line in text.lines() {
        if line.starts_with("HKEY_") {
            flush(&mut current_uuid, &mut current_cert);
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts[0] == "uuid" {
            current_uuid = Some(parts[2].to_owned());
        } else if parts.len() >= 3 && parts[0] == "srvcert" {
            let cert = parts[2..].join(" ");
            if registry_cert_is_usable(&cert) {
                current_cert = Some(cert);
            }
        }
    }
    flush(&mut current_uuid, &mut current_cert);
    found
}

/// The certificate Moonlight holds for a host, if it is paired with it.
pub fn paired_cert(uuid: &str) -> Option<String> {
    for file in settings_files() {
        if let Ok(text) = fs::read_to_string(&file) {
            if let Some(cert) = srvcert_in_ini(&text, uuid) {
                return Some(cert);
            }
        }
    }
    #[cfg(windows)]
    {
        srvcert_in_registry(uuid)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Unique identifier a place announces on its plain-HTTP `/serverinfo` endpoint.
pub fn server_uuid(place: &Place) -> Option<String> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    let addr = (place.address.as_str(), place.port)
        .to_socket_addrs()
        .ok()?
        .next()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    write!(
        stream,
        "GET /serverinfo HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
        place.address
    )
    .ok()?;
    let mut body = String::new();
    stream.take(64 * 1024).read_to_string(&mut body).ok()?;
    let start = body.find("<uniqueid>")? + "<uniqueid>".len();
    let end = body[start..].find("</uniqueid>")? + start;
    Some(body[start..end].trim().to_owned()).filter(|u| !u.is_empty())
}

pub fn stream_args(
    host: &str,
    app: &str,
    s: &Settings,
    native: (u32, u32),
    hz: u32,
) -> Vec<String> {
    let mut args = vec!["stream".into(), host.into(), app.into()];
    let resolution = if s.resolution == "Automatique" {
        Some(native)
    } else if s.resolution == "Personnalisée" {
        Some((s.custom_width, s.custom_height))
    } else {
        s.resolution
            .split_once('×')
            .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
    };
    if let Some((w, h)) = resolution.filter(|(w, h)| *w >= 640 && *h >= 360) {
        args.extend(["--resolution".into(), format!("{w}x{h}")]);
    }
    let fps = if s.fps == "Fréquence écran" {
        hz
    } else if s.fps == "Automatique" {
        0
    } else {
        s.fps.parse().unwrap_or(0)
    };
    if fps > 0 {
        args.extend(["--fps".into(), fps.to_string()]);
    }
    if !s.automatic_bitrate {
        args.extend(["--bitrate".into(), (s.bitrate * 1000).to_string()]);
    }
    let choice =
        |name: &str, value: &str, args: &mut Vec<String>| args.extend([name.into(), value.into()]);
    choice(
        "--display-mode",
        crate::stream_menu::display_mode(s),
        &mut args,
    );
    choice(
        "--video-codec",
        match s.codec.as_str() {
            "H.264" => "H.264",
            "HEVC" => "HEVC",
            "AV1" => "AV1",
            _ => "auto",
        },
        &mut args,
    );
    choice(
        "--video-decoder",
        match s.decoder.as_str() {
            "Matériel" => "hardware",
            "Logiciel" => "software",
            _ => "auto",
        },
        &mut args,
    );
    choice(
        "--audio-config",
        match s.audio.as_str() {
            "5.1" => "5.1-surround",
            "7.1" => "7.1-surround",
            _ => "stereo",
        },
        &mut args,
    );
    choice(
        "--capture-system-keys",
        match s.keyboard_capture.as_str() {
            "Toujours" => "always",
            "Jamais" => "never",
            _ => "fullscreen",
        },
        &mut args,
    );
    for (key, value) in [
        ("vsync", s.vsync),
        ("frame-pacing", s.frame_pacing),
        ("hdr", s.hdr),
        ("yuv444", s.yuv444),
        ("mute-on-focus-loss", s.mute_focus),
        ("background-gamepad", s.background_gamepad),
        ("absolute-mouse", s.remote_mouse),
        ("performance-overlay", s.osd),
        ("audio-on-host", s.play_on_host),
        ("mouse-buttons-swap", s.invert_mouse),
        ("reverse-scroll-direction", s.invert_scroll),
        ("swap-gamepad-buttons", s.swap_gamepad),
        ("multi-controller", s.multiple_gamepads),
        ("keep-awake", s.keep_awake),
        ("game-optimization", s.game_optimization),
    ] {
        args.push(format!("--{}{key}", if value { "" } else { "no-" }));
    }
    if s.quit_after {
        args.push("--quit-after".into());
    }
    args
}

pub fn log_dir() -> io::Result<PathBuf> {
    let base = crate::settings::config_path()
        .and_then(|p| p.parent().map(PathBuf::from))
        .ok_or_else(|| io::Error::other("Dossier de données absent"))?;
    let dir = base.join("logs");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}
fn log() -> io::Result<std::fs::File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir()?.join("moonlight.log"))
}
fn execute(engine: &Engine, args: &[String]) -> io::Result<std::process::Child> {
    let stdout = log()?;
    let stderr = stdout.try_clone()?;
    engine
        .command(args)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
}

// SDL may prefix the line with a timestamp; only the numeric token after this marker matters.
fn termination_code(line: &str) -> Option<i128> {
    let marker = "Connection terminated:";
    let rest = line.split_once(marker)?.1.trim_start();
    let token = rest.split_whitespace().next()?;
    if let Some(hex) = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
    {
        i128::from_str_radix(hex, 16).ok()
    } else {
        token.parse().ok()
    }
}

fn termination_message(place: &str, code: i128, closed_by_host: Option<bool>) -> String {
    if code == 0 || closed_by_host == Some(true) {
        format!("La session sur {place} a été fermée par le PC hôte.")
    } else {
        format!("La connexion à {place} a été perdue.")
    }
}

fn report_termination(place: &Place, code: i128, tx: &Sender<Event>) {
    // Le moteur a déjà arrêté Moonlight. La requête est bornée ici, même si la résolution
    // DNS ou un pair distant dépasse le délai des sockets. L'UI ne bloque jamais.
    let closed = if code != 0 {
        place.session_access.clone().and_then(|access| {
            let (reply_tx, reply_rx) = mpsc::channel();
            thread::spawn(move || {
                let _ = reply_tx.send(crate::join::session_closed_by_host(&access));
            });
            reply_rx.recv_timeout(Duration::from_secs(2)).ok().flatten()
        })
    } else {
        None
    };
    let _ = tx.send(Event::Terminated(termination_message(
        &place.name,
        code,
        closed,
    )));
}

fn capture_output<R: io::Read + Send + 'static>(source: R, tx: Sender<i128>) -> io::Result<()> {
    let mut journal = log()?;
    thread::spawn(move || {
        let mut reader = BufReader::new(source);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let _ = journal.write_all(&line);
                    if let Some(code) = termination_code(&String::from_utf8_lossy(&line)) {
                        let _ = tx.send(code);
                    }
                }
            }
        }
    });
    Ok(())
}

#[derive(Debug)]
pub enum Event {
    Pair(Result<(), String>),
    Started(u32),
    Stopped(String),
    Terminated(String),
}

/// How long a pairing may wait for the code to be confirmed on the host.
const PAIR_TIMEOUT: Duration = Duration::from_secs(300);

/// Error text of a pairing the user abandoned; the interface ignores it.
pub const PAIR_CANCELLED: &str = "Association annulée.";
const ALREADY_PAIRED_UNCONFIRMED: &str = "Association non confirmée : la place signale cet appareil comme déjà associé sans valider ce code. Sur le PC hôte, retirez l'appareil dans Appareils, puis relancez l'association.";

pub fn pair(engine: Engine, place: Place, pin: String, tx: Sender<Event>, cancel: Arc<AtomicBool>) {
    thread::spawn(move || {
        let _ = tx.send(Event::Pair(pair_blocking(&engine, &place, &pin, &cancel)));
    });
}

/// Moonlight's `pair` command exits with 0 whether pairing succeeded or failed, and only after its
/// dialog is dismissed. Success is therefore read from what Moonlight stores: a (new) server
/// certificate for the place's unique id.
fn pair_blocking(
    engine: &Engine,
    place: &Place,
    pin: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let uuid =
        server_uuid(place).ok_or_else(|| format!("La place {} ne répond pas.", place.host()))?;
    // A certificate left from an earlier pairing would look the same after a new one: drop it from
    // Kinseat's own engine settings. Moonlight re-reads the pairing state from the place anyway.
    if let Some(ini) = data_dir().map(|d| d.join(MOONLIGHT_ORG).join("Moonlight.ini")) {
        if let Some(text) = fs::read_to_string(&ini)
            .ok()
            .and_then(|t| without_srvcert(&t, &uuid))
        {
            let _ = fs::write(&ini, text);
        }
    }
    let before = paired_cert(&uuid);
    let log_start = log_dir()
        .ok()
        .and_then(|d| fs::metadata(d.join("moonlight.log")).ok())
        .map_or(0, |m| m.len());
    let new_log = || {
        log_dir()
            .ok()
            .and_then(|d| fs::read(d.join("moonlight.log")).ok())
            .map(|bytes| {
                let start = usize::try_from(log_start).unwrap_or(0).min(bytes.len());
                String::from_utf8_lossy(&bytes[start..]).into_owned()
            })
            .unwrap_or_default()
    };
    let already_paired = || new_log().contains("is already paired");
    // Moonlight reports a failure in a dialog and waits: read it from its log instead.
    let failure = || {
        let log = new_log();
        if log.contains("Incorrect PIN") {
            Some("Code incorrect : la place a refusé le code. Relancez l'association et saisissez le nouveau code sur le PC hôte.".to_owned())
        } else if log.contains("Failed to connect to") {
            Some(format!("Connexion impossible à la place {}.", place.host()))
        } else {
            None
        }
    };
    let args = vec!["pair".into(), place.host(), "--pin".into(), pin.to_owned()];
    let mut child = execute(engine, &args).map_err(|e| format!("Lancement impossible : {e}"))?;
    let started = std::time::Instant::now();
    loop {
        let now = paired_cert(&uuid);
        if now.is_some() && now != before {
            // Paired: close Moonlight's "pairing completed" dialog ourselves.
            thread::sleep(Duration::from_millis(500));
            let _ = child.kill();
            let _ = child.wait();
            return Ok(());
        }
        // "Already paired" is not a success: it proves nothing about this code. Success is only
        // ever a new certificate for this place, confirmed by the host.
        if already_paired() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ALREADY_PAIRED_UNCONFIRMED.into());
        }
        if let Some(message) = failure() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(message);
        }
        // After a wrong code the place drops this request, so a cancel (or a new code) is the
        // only way forward: do not leave Moonlight waiting in the background.
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PAIR_CANCELLED.into());
        }
        match child.try_wait() {
            Ok(Some(_)) => {
                return if paired_cert(&uuid).is_some_and(|c| Some(&c) != before.as_ref()) {
                    Ok(())
                } else {
                    Err("Association non confirmée par la place. Vérifiez le code saisi sur le PC hôte puis réessayez.".into())
                };
            }
            Ok(None) => {}
            Err(e) => return Err(format!("Suivi du processus impossible : {e}")),
        }
        if started.elapsed() > PAIR_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Délai dépassé : le code n'a pas été confirmé sur le PC hôte.".into());
        }
        thread::sleep(Duration::from_millis(500));
    }
}

pub fn stream(engine: Engine, place: Place, args: Vec<String>, tx: Sender<Event>) -> Sender<()> {
    let (stop_tx, stop_rx): (Sender<()>, Receiver<()>) = mpsc::channel();
    thread::spawn(move || {
        // A pipe lets us react before Moonlight presents its error page. Pairing still uses
        // execute() and its direct journal redirection.
        let mut child = match engine
            .command(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                let _ = tx.send(Event::Stopped(format!("Lancement impossible : {e}")));
                return;
            }
        };
        let (term_tx, term_rx) = mpsc::channel();
        let capture = child
            .stdout
            .take()
            .and_then(|out| capture_output(out, term_tx.clone()).ok())
            .is_some()
            && child
                .stderr
                .take()
                .and_then(|err| capture_output(err, term_tx).ok())
                .is_some();
        if !capture {
            let _ = child.kill();
            let _ = child.wait();
            let _ = tx.send(Event::Stopped("Journal Moonlight indisponible".into()));
            return;
        }
        let _ = tx.send(Event::Started(child.id()));
        loop {
            // A deliberate exit takes precedence over any termination line emitted during quit.
            if stop_rx.try_recv().is_ok() {
                // L'UI envoie Ctrl+Alt+Maj+Q à Moonlight. Laisser 5 s pour sa fermeture,
                // puis arrêter uniquement le processus lancé par ce Client.
                let started = std::time::Instant::now();
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            let _ =
                                tx.send(Event::Stopped(format!("Moonlight terminé ({status})")));
                            return;
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Stopped(format!(
                                "Suivi du processus impossible : {e}"
                            )));
                            return;
                        }
                        Ok(None) => {}
                    }
                    if crate::stream_menu::quit_deadline_reached(started.elapsed()) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                let _ = child.kill();
                let _ = child.wait();
                let _ = tx.send(Event::Stopped("Flux arrêté".into()));
                break;
            }
            if let Ok(code) = term_rx.try_recv() {
                let _ = child.kill();
                let _ = child.wait();
                report_termination(&place, code, &tx);
                break;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    // The reader may still be copying the last buffered line after process exit.
                    if let Ok(code) = term_rx.recv_timeout(Duration::from_millis(200)) {
                        report_termination(&place, code, &tx);
                    } else {
                        let _ = tx.send(Event::Stopped(format!("Moonlight terminé ({status})")));
                    }
                    break;
                }
                Err(e) => {
                    let _ = tx.send(Event::Stopped(format!(
                        "Suivi du processus impossible : {e}"
                    )));
                    break;
                }
                Ok(None) => {}
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    stop_tx
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_termination_line() {
        assert_eq!(termination_code("Connection terminated: 0"), Some(0));
        assert_eq!(termination_code("Connection terminated: 42"), Some(42));
        assert_eq!(termination_code("Connection terminated: -101"), Some(-101));
        assert_eq!(
            termination_code("[SDL 17:44:01] Error: Connection terminated: 0x8000FFFF"),
            Some(0x8000FFFF)
        );
        assert_eq!(
            termination_code("Connection terminated: 0xFFFFFFFFFFFFFFFF"),
            Some(0xFFFFFFFFFFFFFFFF)
        );
        assert_eq!(termination_code("Connection started: 0"), None);
        assert_eq!(termination_code("Connection terminated: unknown"), None);
    }

    #[test]
    fn explains_termination_without_exposing_code() {
        let host = "La session sur Place salon a été fermée par le PC hôte.";
        let lost = "La connexion à Place salon a été perdue.";
        assert_eq!(termination_message("Place salon", 0, None), host);
        assert_eq!(termination_message("Place salon", 0, Some(false)), host);
        assert_eq!(termination_message("Place salon", -1, Some(true)), host);
        assert_eq!(termination_message("Place salon", -1, Some(false)), lost);
        assert_eq!(termination_message("Place salon", -101, None), lost);
    }
    /// Real pairing against a running place: `KINSEAT_LIVE_PLACE=host:port KINSEAT_LIVE_PIN=1234
    /// cargo test -p kinseat-client -- --ignored live_pair` while the PIN is submitted on the host.
    #[test]
    #[ignore]
    fn live_pair() {
        let place =
            crate::discovery::parse_address(&std::env::var("KINSEAT_LIVE_PLACE").unwrap()).unwrap();
        let pin = std::env::var("KINSEAT_LIVE_PIN").unwrap();
        let engine = Engine::locate("").expect("moonlight on PATH");
        // KINSEAT_LIVE_CANCEL_AFTER=n: the user presses "Annuler" after n seconds.
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_after = std::env::var("KINSEAT_LIVE_CANCEL_AFTER").ok();
        if let Some(secs) = cancel_after.as_deref().and_then(|s| s.parse().ok()) {
            let flag = cancel.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(secs));
                flag.store(true, Ordering::Relaxed);
            });
        }
        let started = std::time::Instant::now();
        let result = pair_blocking(&engine, &place, &pin, &cancel);
        eprintln!("live_pair result: {result:?} after {:?}", started.elapsed());
        if cancel_after.is_some() {
            assert_eq!(result, Err(PAIR_CANCELLED.to_owned()));
        } else {
            assert!(result.is_ok(), "{result:?}");
        }
    }
    #[test]
    fn reads_the_certificate_of_the_matching_host() {
        let ini = "[General]\nuniqueid=ab\n[hosts]\n1\\uuid=AAA\n1\\srvcert=@ByteArray(-----BEGIN CERTIFICATE-----\\none\\n-----END CERTIFICATE-----)\n2\\uuid=1df69d1e-b8a9\n2\\srvcert=@ByteArray(-----BEGIN CERTIFICATE-----\\ntwo\\n-----END CERTIFICATE-----)\nsize=2\n";
        assert_eq!(
            srvcert_in_ini(ini, "1DF69D1E-B8A9").as_deref(),
            Some("@ByteArray(-----BEGIN CERTIFICATE-----\\ntwo\\n-----END CERTIFICATE-----)")
        );
        assert_eq!(srvcert_in_ini(ini, "zzz"), None);
        let unpaired = "[hosts]\n1\\uuid=AAA\n1\\srvcert=\n";
        assert_eq!(srvcert_in_ini(unpaired, "AAA"), None);
        let discovered = "[hosts]\n1\\srvcert=@ByteArray()\n1\\uuid=AAA\n";
        assert_eq!(srvcert_in_ini(discovered, "AAA"), None);
        let stripped = without_srvcert(ini, "1DF69D1E-B8A9").unwrap();
        assert_eq!(srvcert_in_ini(&stripped, "1DF69D1E-B8A9"), None);
        assert_eq!(
            srvcert_in_ini(&stripped, "AAA").as_deref(),
            Some("@ByteArray(-----BEGIN CERTIFICATE-----\\none\\n-----END CERTIFICATE-----)")
        );
        assert!(stripped.contains("[General]\nuniqueid=ab\n"));
        assert_eq!(without_srvcert(ini, "zzz"), None);
    }
    #[test]
    fn registry_rejects_only_clearly_empty_certificates() {
        assert!(!registry_cert_is_usable("@ByteArray()"));
        assert!(!registry_cert_is_usable(
            "@ByteArray(-----BEGIN CERTIFICATE-----\\n-----END CERTIFICATE-----\\n)"
        ));
        let empty_hex = "-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n"
            .bytes()
            .fold(String::new(), |mut acc, b| {
                use std::fmt::Write;
                let _ = write!(acc, "{b:02X}");
                acc
            });
        assert!(!registry_cert_is_usable(&empty_hex));
        assert!(registry_cert_is_usable("2D2D2D2D2D424547494E"));
        assert!(registry_cert_is_usable(
            "@ByteArray(-----BEGIN CERTIFICATE-----\\nYWJjZA==\\n-----END CERTIFICATE-----)"
        ));
    }
    #[test]
    fn accepts_only_nonempty_pem_certificates() {
        let empty = "-----BEGIN CERTIFICATE-----\\n-----END CERTIFICATE-----";
        let valid = "-----BEGIN CERTIFICATE-----\\nYWJjZA==\\n-----END CERTIFICATE-----";
        assert!(!is_real_pem_cert(empty));
        assert!(is_real_pem_cert(valid));
        assert!(!is_real_pem_cert("@ByteArray()"));

        // Exact QSettings placeholder emitted by Moonlight while pairing is incomplete.
        let ini = "[hosts]\n1\\uuid=AAA\n1\\srvcert=@ByteArray(-----BEGIN CERTIFICATE-----\\n-----END CERTIFICATE-----\\n)\n";
        assert_eq!(srvcert_in_ini(ini, "AAA"), None);
        let ini = format!("[hosts]\n1\\uuid=AAA\n1\\srvcert=@ByteArray({valid}\\n)\n");
        assert!(srvcert_in_ini(&ini, "AAA").is_some());
        let empty_ini = "[hosts]\n1\\uuid=AAA\n1\\srvcert=@ByteArray()\n";
        assert_eq!(srvcert_in_ini(empty_ini, "AAA"), None);
    }

    #[test]
    fn stream_options() {
        let s = Settings {
            automatic_bitrate: false,
            bitrate: 25,
            display_mode: "Fenêtré sans bordure".into(),
            ..Default::default()
        };
        let a = stream_args("host:48100", "Desktop", &s, (2560, 1600), 60);
        assert_eq!(&a[..3], ["stream", "host:48100", "Desktop"]);
        assert!(a.windows(2).any(|w| w == ["--resolution", "2560x1600"]));
        assert!(a.windows(2).any(|w| w == ["--bitrate", "25000"]));
        assert!(a.windows(2).any(|w| w == ["--display-mode", "borderless"]));
        assert!(a.contains(&"--no-hdr".to_owned()));
        assert!(a.contains(&"--keep-awake".to_owned()));
        assert!(a.contains(&"--no-audio-on-host".to_owned()));
        assert!(!a.contains(&"--quit-after".to_owned()));
    }
}
