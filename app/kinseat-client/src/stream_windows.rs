//! Intégration Windows du panneau. Aucun enregistrement ni conservation de frappes.
#![cfg(windows)]
use crate::stream_menu;
use std::sync::{mpsc, Mutex, OnceLock};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, RECT, WPARAM},
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    System::Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        },
        WindowsAndMessaging::{
            AllowSetForegroundWindow, CallNextHookEx, EnumWindows, GetClipCursor, GetCursorInfo,
            GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindowTextW,
            GetWindowThreadProcessId, IsWindowVisible, KillTimer, PeekMessageW, PostThreadMessageW,
            SetForegroundWindow, SetTimer, SetWindowPos, SetWindowsHookExW, UnhookWindowsHookEx,
            CURSORINFO, CURSOR_SHOWING, HWND_TOPMOST, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
            PM_NOREMOVE, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SWP_NOMOVE, SWP_NOSIZE,
            SWP_SHOWWINDOW, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN,
            WM_SYSKEYUP, WM_TIMER,
        },
    },
};

struct HookState {
    pid: u32,
    panel: isize,
    shortcut: String,
    sender: mpsc::Sender<()>,
    repaint: egui::Context,
    pending_open: bool,
    panel_foreground: bool,
    focus_failed: bool,
    keys: stream_menu::ShortcutKeys,
}
// Le crochet est unique pour ce processus. La fermeture du flux retire le hook et son état.
static HOOK: OnceLock<Mutex<Option<HookState>>> = OnceLock::new();
fn state() -> &'static Mutex<Option<HookState>> {
    HOOK.get_or_init(|| Mutex::new(None))
}

unsafe extern "system" fn keyboard(code: i32, message: WPARAM, data: LPARAM) -> isize {
    if code >= 0 && data != 0 {
        let key = unsafe { &*(data as *const KBDLLHOOKSTRUCT) };
        if key.flags & LLKHF_INJECTED == 0 {
            if let Ok(mut guard) = state().try_lock() {
                if let Some(s) = guard.as_mut() {
                    let down = message as u32 == WM_KEYDOWN || message as u32 == WM_SYSKEYDOWN;
                    let up = message as u32 == WM_KEYUP || message as u32 == WM_SYSKEYUP;
                    let fg = unsafe { GetForegroundWindow() };
                    let mut pid = 0;
                    unsafe { GetWindowThreadProcessId(fg, &mut pid) };
                    let own = fg as isize == s.panel && !fg.is_null();
                    let active = pid == s.pid || own;
                    if down || up {
                        let decision = s.keys.event(&s.shortcut, key.vkCode, down, active);
                        if decision.trigger {
                            // Ctrl pendant Alt est transmis par Moonlight : évite que
                            // Alt↑ seul active le menu du PC hôte. K ne lui est pas livré.
                            neutral_key();
                            crate::engine::diagnostic("raccourci du menu reçu");
                            let _ = s.sender.send(());
                            s.repaint.request_repaint();
                            s.pending_open = true;
                        }
                        if s.pending_open && s.keys.idle() {
                            s.pending_open = false;
                            // Réveiller l'UI seulement après Alt↑, pour ne pas voler
                            // son relâchement à Moonlight.
                            unsafe { SetTimer(std::ptr::null_mut(), 2, 50, None) };
                        }
                        if decision.swallow {
                            return 1;
                        }
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, message, data) }
}

fn neutral_key() {
    // 0xE8 masque le menu local ; Ctrl est une vraie touche que Moonlight relaie à l'hôte.
    let inputs = [
        keyboard_input(0xE8, false),
        keyboard_input(0x11, false),
        keyboard_input(0x11, true),
        keyboard_input(0xE8, true),
    ];
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
}
fn keyboard_input(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                ..Default::default()
            },
        },
    }
}

pub struct ShortcutHook {
    thread_id: u32,
    handle: Option<std::thread::JoinHandle<()>>,
}
impl ShortcutHook {
    pub fn start(
        pid: u32,
        shortcut: &str,
        sender: mpsc::Sender<()>,
        repaint: egui::Context,
    ) -> Option<Self> {
        if shortcut == "Désactivé" {
            return None;
        }
        let (ready_tx, ready_rx) = mpsc::channel();
        let name = shortcut.to_owned();
        let handle = std::thread::spawn(move || {
            let thread_id = unsafe { GetCurrentThreadId() };
            let mut msg = MSG::default();
            unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
            let hook = unsafe {
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), std::ptr::null_mut(), 0)
            };
            if hook.is_null() {
                let _ = ready_tx.send(None);
                return;
            }
            *state().lock().expect("hook mutex") = Some(HookState {
                pid,
                panel: 0,
                shortcut: name,
                sender,
                repaint,
                pending_open: false,
                panel_foreground: false,
                focus_failed: false,
                keys: stream_menu::ShortcutKeys::default(),
            });
            crate::engine::diagnostic("crochet installé");
            let timer = unsafe { SetTimer(std::ptr::null_mut(), 1, 2000, None) };
            let _ = ready_tx.send(Some(thread_id));
            let mut hook = hook;
            let mut lost = false;
            while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
                if msg.message == WM_TIMER && msg.wParam == 2 {
                    unsafe { KillTimer(std::ptr::null_mut(), 2) };
                    if let Some(s) = state().lock().expect("hook mutex").as_ref() {
                        s.repaint.request_repaint();
                    }
                }
                if msg.message == WM_TIMER && msg.wParam == timer {
                    let mut guard = state().lock().expect("hook mutex");
                    if let Some(s) = guard.as_mut() {
                        let fg = unsafe { GetForegroundWindow() };
                        let mut owner = 0;
                        unsafe { GetWindowThreadProcessId(fg, &mut owner) };
                        if owner == s.pid && s.keys.idle() {
                            let replacement = unsafe {
                                SetWindowsHookExW(
                                    WH_KEYBOARD_LL,
                                    Some(keyboard),
                                    std::ptr::null_mut(),
                                    0,
                                )
                            };
                            if !replacement.is_null() {
                                unsafe { UnhookWindowsHookEx(hook) };
                                hook = replacement;
                                if lost {
                                    lost = false;
                                    crate::engine::diagnostic("crochet réinstallé");
                                }
                            } else if !lost {
                                lost = true;
                                crate::engine::diagnostic(
                                    "crochet perdu : réinstallation impossible",
                                );
                            }
                        } else if owner != s.pid && fg as isize != s.panel {
                            s.keys = stream_menu::ShortcutKeys::default();
                        }
                    }
                }
            }
            if timer != 0 {
                unsafe { KillTimer(std::ptr::null_mut(), timer) };
            }
            if !hook.is_null() {
                unsafe { UnhookWindowsHookEx(hook) };
            }
            *state().lock().expect("hook mutex") = None;
        });
        match ready_rx.recv().ok().flatten() {
            Some(thread_id) => Some(Self {
                thread_id,
                handle: Some(handle),
            }),
            None => {
                let _ = handle.join();
                None
            }
        }
    }
    pub fn set_panel(&self, panel: HWND) -> bool {
        let Ok(mut guard) = state().lock() else {
            return false;
        };
        let Some(s) = guard.as_mut() else {
            return false;
        };
        if panel.is_null() {
            s.panel = 0;
            s.panel_foreground = false;
            s.focus_failed = false;
            return true;
        }
        if s.panel != panel as isize {
            s.panel = panel as isize;
            s.panel_foreground = false;
            s.focus_failed = false;
            crate::engine::diagnostic("fenêtre du panneau créée");
        }
        if s.panel_foreground {
            return true;
        }
        if s.pending_open || !s.keys.idle() {
            return false; // Alt↑ doit encore atteindre Moonlight.
        }
        let shown = unsafe {
            SetWindowPos(
                panel,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            )
        } != 0;
        let focused = shown && focus_window(panel);
        if focused {
            s.panel_foreground = true;
            crate::engine::diagnostic("panneau au premier plan");
        } else if !s.focus_failed {
            s.focus_failed = true;
            crate::engine::diagnostic("échec premier plan");
        }
        focused
    }
}
impl Drop for ShortcutHook {
    fn drop(&mut self) {
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct WindowSearch {
    pid: u32,
    window: HWND,
}
unsafe extern "system" fn find_window(hwnd: HWND, data: LPARAM) -> i32 {
    let search = unsafe { &mut *(data as *mut WindowSearch) };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == search.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
        search.window = hwnd;
        return 0;
    }
    1
}
pub fn moonlight_window(pid: u32) -> HWND {
    let mut found = WindowSearch {
        pid,
        window: std::ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(find_window), &mut found as *mut WindowSearch as isize) };
    found.window
}
struct PanelSearch {
    window: HWND,
}
unsafe extern "system" fn find_panel(hwnd: HWND, data: LPARAM) -> i32 {
    let found = unsafe { &mut *(data as *mut PanelSearch) };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid != unsafe { GetCurrentProcessId() } {
        return 1;
    }
    let mut title = [0u16; 80];
    let count = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
    if count > 0 && String::from_utf16_lossy(&title[..count as usize]) == "Kinseat — flux" {
        found.window = hwnd;
        return 0;
    }
    1
}
fn focus_window(hwnd: HWND) -> bool {
    if unsafe { SetForegroundWindow(hwnd) } != 0 && unsafe { GetForegroundWindow() } == hwnd {
        return true;
    }
    let foreground = unsafe { GetForegroundWindow() };
    let other = unsafe { GetWindowThreadProcessId(foreground, std::ptr::null_mut()) };
    let current = unsafe { GetCurrentThreadId() };
    if other != 0 && other != current && unsafe { AttachThreadInput(current, other, 1) } != 0 {
        let focused =
            unsafe { SetForegroundWindow(hwnd) } != 0 && unsafe { GetForegroundWindow() } == hwnd;
        unsafe { AttachThreadInput(current, other, 0) };
        return focused;
    }
    false
}

pub fn panel_window() -> HWND {
    let mut found = PanelSearch {
        window: std::ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(find_panel), &mut found as *mut PanelSearch as isize) };
    found.window
}

pub fn monitor_rect(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
    if hwnd.is_null() {
        return None;
    }
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: Default::default(),
        rcWork: Default::default(),
        dwFlags: 0,
    };
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then(|| {
        let r = info.rcMonitor;
        (r.left, r.top, r.right, r.bottom)
    })
}
pub fn focus_moonlight(pid: u32) -> bool {
    let hwnd = moonlight_window(pid);
    if hwnd.is_null() {
        return false;
    }
    // Le panneau appartient au Client au premier plan : lui céder explicitement le droit.
    unsafe { AllowSetForegroundWindow(pid) };
    if unsafe { SetForegroundWindow(hwnd) } != 0 {
        return true;
    }
    // Si le viewport et l'UI tournent sur des threads différents, partager temporairement
    // la file d'entrée du thread au premier plan, puis toujours la détacher.
    let foreground = unsafe { GetForegroundWindow() };
    let foreground_thread = unsafe { GetWindowThreadProcessId(foreground, std::ptr::null_mut()) };
    let current_thread = unsafe { GetCurrentThreadId() };
    if foreground_thread != 0
        && foreground_thread != current_thread
        && unsafe { AttachThreadInput(current_thread, foreground_thread, 1) } != 0
    {
        let focused = unsafe { SetForegroundWindow(hwnd) } != 0;
        unsafe { AttachThreadInput(current_thread, foreground_thread, 0) };
        return focused;
    }
    false
}
pub fn send_shortcut(pid: u32, key: u16) -> bool {
    if !focus_moonlight(pid) {
        return false;
    }
    let hwnd = moonlight_window(pid);
    if hwnd.is_null() {
        return false;
    }
    // SetForegroundWindow peut réussir avant que le changement de focus soit visible.
    // Ne jamais injecter la combinaison dans le panneau ou une autre application.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while unsafe { GetForegroundWindow() } != hwnd {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let mut input = Vec::with_capacity(8);
    for vk in [0x11u16, 0x12, 0x10, key] {
        input.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    ..Default::default()
                },
            },
        });
    }
    for vk in [key, 0x10, 0x12, 0x11] {
        input.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    dwFlags: KEYEVENTF_KEYUP,
                    ..Default::default()
                },
            },
        });
    }
    unsafe {
        SendInput(
            input.len() as u32,
            input.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        ) == input.len() as u32
    }
}

/// Souris encore capturée par Moonlight : curseur confiné à une zone plus petite que le bureau
/// virtuel, ou masqué.
fn mouse_captured() -> bool {
    let mut clip: RECT = unsafe { std::mem::zeroed() };
    let clipped = unsafe { GetClipCursor(&mut clip) } != 0 && {
        let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        clip.right - clip.left < width || clip.bottom - clip.top < height
    };
    let mut info: CURSORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
    let hidden = unsafe { GetCursorInfo(&mut info) } != 0 && (info.flags & CURSOR_SHOWING) == 0;
    stream_menu::release_needs_toggle(clipped, hidden)
}

/// « Libérer la souris » : le raccourci Z est un toggle, il n'est envoyé que si la souris est
/// encore capturée une fois Moonlight revenu au premier plan. Renvoie vrai si la souris est libre.
pub fn release_mouse(pid: u32) -> bool {
    let hwnd = moonlight_window(pid);
    if hwnd.is_null() || !focus_moonlight(pid) {
        return false;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while unsafe { GetForegroundWindow() } != hwnd {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Laisser Moonlight traiter le gain de focus (il peut recapturer à ce moment-là).
    std::thread::sleep(std::time::Duration::from_millis(150));
    if !mouse_captured() {
        return true;
    }
    if !send_shortcut(pid, b'Z' as u16) {
        return false;
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
    !mouse_captured()
}
