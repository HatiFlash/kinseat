//! Sondes Windows exclusivement en lecture : API disque, interrogations reg/sc/netstat/pnputil.
use crate::{Port, Service, SystemFacts};
use std::{collections::HashMap, process::Command};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

#[link(name = "shell32")]
extern "system" {
    fn IsUserAnAdmin() -> i32;
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW : pas de console qui clignote pendant les vérifications du setup.
    let result = Command::new(program)
        .args(args)
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    result
        .status
        .success()
        .then(|| String::from_utf8_lossy(&result.stdout).into_owned())
}
fn reg(key: &str, value: &str) -> Option<String> {
    let text = output("reg", &["query", key, "/v", value])?;
    text.lines()
        .find(|line| line.trim_start().starts_with(value))
        .and_then(|line| line.split_whitespace().nth(2))
        .map(str::to_owned)
}
fn key_exists(key: &str) -> bool {
    output("reg", &["query", key]).is_some()
}
fn service(name: &str) -> Service {
    let text = output("sc", &["query", name]);
    Service {
        present: text.is_some(),
        actif: text.is_some_and(|s| {
            s.lines().any(|l| {
                (l.contains("STATE") || l.contains("ÉTAT") || l.contains("ETAT"))
                    && (l.contains(": 4 ") || l.trim_end().ends_with(": 4"))
            })
        }),
    }
}
fn ports() -> (Vec<Port>, bool) {
    let owners: HashMap<String, String> = output("tasklist", &["/fo", "csv", "/nh"])
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.trim_matches('"').split("\",\"").collect();
            (fields.len() > 1).then(|| (fields[1].to_owned(), fields[0].to_owned()))
        })
        .collect();
    let text = match output("netstat", &["-ano"]) {
        Some(text) => text,
        None => return (Vec::new(), false),
    };
    let found = text
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            let proto = *fields.first()?;
            if proto != "TCP" && proto != "UDP" {
                return None;
            }
            // TCP LISTENING seulement ; UDP n'a pas d'état « en écoute » dans netstat.
            if proto == "TCP" && !fields.iter().any(|s| s.eq_ignore_ascii_case("LISTENING")) {
                return None;
            }
            let numero = fields.get(1)?.rsplit(':').next()?.parse().ok()?;
            let pid = *fields.last()?;
            Some(Port {
                numero,
                protocole: proto.into(),
                processus: owners.get(pid).cloned(),
            })
        })
        .collect();
    (found, true)
}
fn disk_gb() -> Option<u64> {
    let drive = std::env::var("SystemDrive").ok()?;
    let path: Vec<u16> = format!("{drive}\\").encode_utf16().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: chemin UTF-16 nul-terminé et pointeur de sortie valide.
    (unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } != 0)
        .then_some(free / 1_000_000_000)
}
pub(super) fn collect() -> SystemFacts {
    let current = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = reg(current, "CurrentBuildNumber").and_then(|v| v.parse().ok());
    let gpu = output("pnputil", &["/enum-devices", "/class", "Display"]).and_then(|text| {
        text.lines()
            .map(str::trim)
            .find(|l| {
                ["NVIDIA", "AMD", "Intel"]
                    .iter()
                    .any(|m| l.to_ascii_lowercase().contains(&m.to_ascii_lowercase()))
            })
            // « Description de l'appareil : NVIDIA Quadro M4000 » -> « NVIDIA Quadro M4000 »
            .map(|l| {
                l.split_once(':')
                    .map_or(l, |(_, name)| name)
                    .trim()
                    .to_owned()
            })
    });
    let reboot_pending = key_exists(
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending",
    ) || key_exists(
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired",
    ) || reg(
        r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager",
        "PendingFileRenameOperations",
    )
    .is_some();
    let apollo = service("ApolloService");
    let sunshine = service("SunshineService");
    let (ports, ports_known) = ports();
    SystemFacts {
        windows_major: build.map(|_| 10),
        build,
        edition: reg(current, "EditionID"),
        admin: Some(unsafe { IsUserAnAdmin() != 0 }),
        term_service: service("TermService"),
        vigem_bus: service("ViGEmBus"),
        hid_hide: service("HidHide"),
        apollo: Service {
            present: apollo.present || sunshine.present,
            actif: apollo.actif || sunshine.actif,
        },
        multiseat: service("MultiSeatService"),
        ports,
        ports_known,
        disque_go: disk_gb(),
        gpu,
        reboot_pending: Some(reboot_pending),
        rdpwrap: Some(
            key_exists(r"HKLM\SOFTWARE\RDP Wrapper")
                || std::path::Path::new(r"C:\Program Files\RDP Wrapper\rdpwrap.dll").exists(),
        ),
    }
}
