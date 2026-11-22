use kinseat_ipc::LogonTask;

pub fn key(task: &LogonTask) -> String {
    format!("{}{}", task.path, task.name)
}

pub fn name(task: &LogonTask) -> String {
    if let Some(name) = task
        .display_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        return name.to_owned();
    }
    let exe = task.execute.trim().trim_matches('"');
    let exe = exe
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(exe)
        .trim_matches('"');
    let exe = exe
        .get(..exe.len().saturating_sub(4))
        .filter(|_| exe.to_ascii_lowercase().ends_with(".exe"))
        .unwrap_or(exe);
    if exe.is_empty() {
        task.name.clone()
    } else {
        exe.to_owned()
    }
}

pub fn sort(tasks: &mut [LogonTask]) {
    tasks.sort_by(|a, b| {
        let rank = |t: &LogonTask| match t.kind.as_deref() {
            Some("overlay") => 0,
            Some("oneTime") => 2,
            _ => 1,
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| name(a).to_lowercase().cmp(&name(b).to_lowercase()))
    });
}

pub fn count_label(n: usize) -> String {
    if n == 1 {
        "1 programme s'ouvre sur votre écran à chaque démarrage de place".into()
    } else {
        format!("{n} programmes s'ouvrent sur votre écran à chaque démarrage de place")
    }
}

pub fn visible(tasks: &[LogonTask], dismissed: &[String]) -> bool {
    let keys: Vec<_> = tasks.iter().filter(|t| !t.restricted).map(key).collect();
    !keys.is_empty() && keys.iter().any(|k| !dismissed.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task(name: &str, kind: Option<&str>) -> LogonTask {
        LogonTask {
            path: "\\".into(),
            name: name.into(),
            execute: format!("\"C:\\Tools\\{name}.exe\""),
            run_as: "alice".into(),
            restricted: false,
            display_name: None,
            exe_name: None,
            kind: kind.map(str::to_owned),
        }
    }
    #[test]
    fn singular_plural_and_fallback() {
        assert!(count_label(1).starts_with("1 programme s'ouvre"));
        assert!(count_label(6).starts_with("6 programmes s'ouvrent"));
        assert_eq!(name(&task("HWiNFO", None)), "HWiNFO");
        let mut legacy = task("nom tâche", None);
        legacy.execute = "\"C:\\Program Files\\Tools\\MyTool.ExE\"".into();
        assert_eq!(name(&legacy), "MyTool");
        legacy.display_name = Some("Outil lisible".into());
        assert_eq!(name(&legacy), "Outil lisible");
    }
    #[test]
    fn category_then_alpha() {
        let mut tasks = vec![
            task("z", Some("other")),
            task("B", Some("overlay")),
            task("a", Some("overlay")),
            task("c", Some("oneTime")),
        ];
        sort(&mut tasks);
        assert_eq!(
            tasks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["a", "B", "z", "c"]
        );
    }
    #[test]
    fn dismissed_only_until_new_task() {
        let a = task("a", None);
        assert!(visible(std::slice::from_ref(&a), &[]));
        assert!(!visible(std::slice::from_ref(&a), &[key(&a)]));
        assert!(visible(&[a.clone(), task("b", None)], &[key(&a)]));
        let mut limited = a;
        limited.restricted = true;
        assert!(!visible(&[limited], &[]));
    }
}
