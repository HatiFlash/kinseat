use kinseat_ipc::{GpuBudget, Health, Library, LighteningReport, LogonTask, Notice, Seat};

pub fn logon_tasks() -> Vec<LogonTask> {
    serde_json::from_value(serde_json::json!([
        {"path":"\\HWiNFO\\", "name":"HWiNFO", "execute":"C:\\Program Files\\HWiNFO64\\HWiNFO64.EXE", "exeName":"HWiNFO64.EXE", "displayName":"HWiNFO", "runAs":"CAMILLE", "kind":"overlay", "restricted":false},
        {"path":"\\", "name":"MSIAfterburner", "execute":"C:\\Program Files (x86)\\MSI Afterburner\\MSIAfterburner.exe", "exeName":"MSIAfterburner.exe", "displayName":"MSI Afterburner", "runAs":"CAMILLE", "kind":"overlay", "restricted":false},
        {"path":"\\", "name":"RTSS", "execute":"C:\\Program Files (x86)\\RivaTuner Statistics Server\\RTSS.exe", "exeName":"RTSS.exe", "displayName":"RivaTuner Statistics Server", "runAs":"CAMILLE", "kind":"overlay", "restricted":false},
        {"path":"\\HWiNFO\\", "name":"Start HWiNFO SharedMemory", "execute":"C:\\Program Files\\HWiNFO64\\HWiNFO64.EXE", "exeName":"HWiNFO64.EXE", "displayName":"HWiNFO SharedMemory", "runAs":"CAMILLE", "kind":"overlay", "restricted":false},
        {"path":"\\IObit\\", "name":"IObit SUM2026Sale", "execute":"C:\\Program Files (x86)\\IObit\\Software Updater\\IObitSoftwareUpdater.exe", "exeName":"IObitSoftwareUpdater.exe", "displayName":"IObit Software Updater", "runAs":"CAMILLE", "kind":"oneTime", "restricted":false},
        {"path":"\\", "name":"Jungle_Leopard_Display", "execute":"C:\\Program Files\\Jungle Leopard\\Display.exe", "exeName":"Display.exe", "displayName":"Jungle Leopard Display", "runAs":"CAMILLE", "kind":"other", "restricted":false}
    ])).expect("tâches fictives")
}

pub fn budget() -> GpuBudget {
    budget_with_reserve(2048)
}
pub fn budget_with_reserve(reserve: u64) -> GpuBudget {
    serde_json::from_value(serde_json::json!({"gpuName":"RTX 3080 · démonstration","totalMb":10240,"hostReserveMb":reserve,"usableMb":10240_u64.saturating_sub(reserve),"allocatedMb":6144,"freeMb":10240_u64.saturating_sub(reserve).saturating_sub(6144),"seats":[]})).expect("budget fictif")
}
pub fn lightening_report() -> LighteningReport {
    serde_json::from_value(serde_json::json!({"settings":{"emptyDesktop":true,"blockStartupApps":true,"blockVendorUtilities":true,"closeWindhawkTray":true},"stoppedProcesses":[{"name":"iCUE.exe","category":"utilitaire constructeur","at":"2026-11-18T18:05:00Z"}],"blockedStartupEntries":["Utilitaire constructeur"]})).expect("rapport fictif")
}
pub fn library() -> Library {
    serde_json::from_value(serde_json::json!({"scannedAt":"2026-11-18T18:00:00Z","games":[{"id":"steam:demo","name":"Jeu familial (fictif)","source":"Steam","installPath":"C:\\Jeux\\Exemple","launchUri":"steam://rungameid/demo","assignedTo":["Camille"]},{"id":"manual:demo","name":"Jeu coopératif (fictif)","source":"manual","installPath":"C:\\Jeux\\Exemple2","exePath":"C:\\Jeux\\Exemple2\\jeu.exe","assignedTo":["Yanis","Léa"]}]})).expect("bibliothèque fictive")
}

pub fn notices() -> Vec<Notice> {
    serde_json::from_value(serde_json::json!([
        {"id":"demo-active","seatId":"2","accountName":"Yanis","message":"La session va se terminer. Pense à sauvegarder ta partie.","minutes":10,"createdAtUtc":"2026-11-13T18:10:00Z","deadlineUtc":"2026-11-13T18:20:00Z","remainingSeconds":252,"phase":"Displayed"},
        {"id":"demo-closed","seatId":"3","accountName":"Léa","message":"La session va se terminer.","minutes":5,"createdAtUtc":"2026-11-13T17:00:00Z","deadlineUtc":"2026-11-13T17:05:00Z","remainingSeconds":0,"phase":"Closed"}
    ])).expect("fixture préavis")
}

pub fn data() -> (Health, Vec<Seat>) {
    let health = serde_json::from_value(serde_json::json!({
        "activeSeats":3,"maxSeats":4,
        "health":{"status":"Warning","score":85,"issues":["CPU high: 78%"]},
        "windowsBuild":"Windows 11 · 26100","rdpWrapperActive":true,
        "gpu":{"name":"NVIDIA GeForce RTX 3080","utilizationPercent":62,"vramTotalMb":10240,"vramUsedMb":6300,"encoderUtilizationPercent":38,"activeEncoderSessions":2},
        "cpu":{"name":"AMD Ryzen 7 5800X","utilizationPercent":78,"logicalCores":16},
        "network":{"primaryAdapter":"Ethernet","bytesReceivedPerSec":3200000,"bytesSentPerSec":14800000},
        "disks":[{"name":"C:","label":"Système","totalMb":1024000,"freeMb":348000,"usedPercent":66}],
        "systemMemoryMb":32768,"availableMemoryMb":14336,
        "topProcesses":[{"pid":42,"name":"Jeu","cpuPercent":25.0,"memoryMb":2600,"networkConnections":3}]
    })).expect("fixture santé");
    let seats = serde_json::from_value(serde_json::json!([
        {"id":"1","accountName":"Camille","sessionId":3,"status":"Streaming","width":1920,"height":1080,"fps":60,"portBase":48100,"launchApp":"Rocket League","createdAt":"2026-11-13T18:00:00Z","autoStart":true,"viGEmControllerIndex":0,"graphicsProfile":"Balanced","vramBudgetMb":3072,"vramUsedMb":2048,"lightening":{"emptyDesktop":true,"blockStartupApps":true,"blockVendorUtilities":true,"closeWindhawkTray":true}},
        {"id":"2","accountName":"Yanis","sessionId":4,"status":"Streaming","width":2560,"height":1440,"fps":60,"portBase":48130,"launchApp":"Minecraft","createdAt":"2026-11-13T18:10:00Z","autoStart":true,"viGEmControllerIndex":1},
        {"id":"3","accountName":"Léa","sessionId":5,"status":"Ready","width":1920,"height":1080,"fps":60,"portBase":48160,"createdAt":"2026-11-13T18:15:00Z","autoStart":false}
    ])).expect("fixture sièges");
    (health, seats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_logon_tasks_are_six_distinct_entries() {
        let tasks = logon_tasks();
        assert_eq!(
            tasks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            [
                "HWiNFO",
                "MSIAfterburner",
                "RTSS",
                "Start HWiNFO SharedMemory",
                "IObit SUM2026Sale",
                "Jungle_Leopard_Display"
            ]
        );
        assert!(tasks
            .iter()
            .all(|t| !t.restricted && t.display_name.is_some()));
    }
}
