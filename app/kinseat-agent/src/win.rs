use super::urgent;
use crate::layout::notice_layout;
use crate::notice_state::{render_changed, Mode, NoticeState, Presentation, RenderSnapshot};
use crate::protocol::{self, Incoming, Outgoing};
use base64::Engine;
use std::{
    fs::{self, OpenOptions},
    io::{BufReader, BufWriter, Write},
    sync::{mpsc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::*, RemoteDesktop::*, Threading::*},
    UI::{HiDpi::*, WindowsAndMessaging::*},
};

// Per-user log: a seat account cannot write service-owned ProgramData files.
// Never put notice text, image data or pipe contents here.
/// Reads the agent pipe without ever leaving a ReadFile pending on an empty pipe.
struct PeekReader(std::fs::File);

impl std::io::Read for PeekReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Pipes::PeekNamedPipe;
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let mut available = 0u32;
            let ok = unsafe {
                PeekNamedPipe(
                    self.0.as_raw_handle() as _,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                // ERROR_BROKEN_PIPE (109): the service closed its end.
                return if err.raw_os_error() == Some(109) {
                    Ok(0)
                } else {
                    Err(err)
                };
            }
            if available > 0 {
                let n = buf.len().min(available as usize);
                return std::io::Read::read(&mut self.0, &mut buf[..n]);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

static LOG_LOCK: Mutex<()> = Mutex::new(());
pub fn log(event: &str) {
    let Ok(_guard) = LOG_LOCK.lock() else { return };
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|p| std::path::PathBuf::from(p).join("AppData").join("Local"))
        });
    let Some(dir) = dir else { return };
    let dir = dir.join("Kinseat");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("agent.log");
    let line = format!("{} {event}\n", utc_now());
    if line.len() > 1024 * 1024 {
        return;
    }
    if fs::metadata(&path).is_ok_and(|m| m.len() + line.len() as u64 > 1024 * 1024) {
        let backup = dir.join("agent.log.1");
        if fs::rename(&path, &backup).is_err() {
            let _ = fs::remove_file(&backup);
            if fs::rename(&path, &backup).is_err() {
                return;
            }
        }
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn failure() -> String {
    std::io::Error::last_os_error().to_string()
}

// SetLayeredWindowAttributes uses constant opacity, not per-pixel alpha: the DIB
// is already composited against the same opaque background as FillRect.
#[allow(clippy::too_many_arguments)]
unsafe fn paint_ring(
    dc: HDC,
    x: i32,
    y: i32,
    size: i32,
    radius: f64,
    stroke: f64,
    fraction: f64,
    track: [u8; 3],
    active: [u8; 3],
) {
    let size = size.max(1);
    let pixels = crate::ring::raster(
        size as usize,
        size as usize,
        size as f64 / 2.0,
        size as f64 / 2.0,
        radius,
        stroke,
        fraction,
        track,
        active,
        [0x1c, 0x1a, 0x18],
    );
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = size;
    info.bmiHeader.biHeight = -size;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    StretchDIBits(
        dc,
        x,
        y,
        size,
        size,
        0,
        0,
        size,
        size,
        pixels.as_ptr().cast(),
        &info,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
}

struct Overlay {
    hwnd: HWND,
    title: String,
    message: String,
    seconds: u64,
    since: Instant,
    presentation_clock: Instant,
    badge: bool,
    state: Option<NoticeState>,
    presentation: Presentation,
    speed: u64,
    geometry: Option<(i32, i32, i32, i32, i32, i32, i32)>,
    last_render: Option<RenderSnapshot>,
}
impl Overlay {
    fn new(badge: bool) -> Result<Box<Self>, String> {
        unsafe {
            let cls = wide(if badge {
                "KinseatBadge"
            } else {
                "KinseatNotice"
            });
            let instance = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                lpszClassName: cls.as_ptr(),
                ..std::mem::zeroed()
            };
            if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(failure());
            }
            let scale = GetDpiForSystem() as i32;
            let mul = |n: i32| n * scale / 96;
            let w = mul(if badge { 190 } else { 300 });
            let h = mul(if badge { 48 } else { 164 });
            let x = if badge {
                GetSystemMetrics(SM_CXSCREEN) - w - mul(20)
            } else {
                mul(20)
            };
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_NOACTIVATE
                    | WS_EX_TRANSPARENT
                    | WS_EX_LAYERED
                    | WS_EX_TOOLWINDOW,
                cls.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                x,
                mul(20),
                w,
                h,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return Err(failure());
            }
            if SetLayeredWindowAttributes(hwnd, 0, 235, LWA_ALPHA) == 0 {
                DestroyWindow(hwnd);
                return Err(failure());
            }
            let mut overlay = Box::new(Self {
                hwnd,
                title: if badge {
                    "Aperçu en cours"
                } else {
                    "Fermeture de la session"
                }
                .into(),
                message: String::new(),
                seconds: 0,
                since: Instant::now(),
                presentation_clock: Instant::now(),
                badge,
                state: None,
                presentation: Presentation {
                    mode: Mode::Large,
                    fraction: 1.0,
                    urgent: false,
                },
                speed: 1,
                geometry: None,
                last_render: None,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&mut *overlay as *mut Self) as isize);
            overlay.resize(if badge { Mode::Capsule } else { Mode::Large });
            Ok(overlay)
        }
    }
    fn remaining_now(&self, now: Instant) -> u64 {
        self.seconds.saturating_sub(
            now.duration_since(self.since)
                .as_secs()
                .saturating_mul(self.speed),
        )
    }
    fn tick(&mut self) {
        if self.badge {
            return;
        }
        let now = Instant::now();
        let secs = self.remaining_now(now);
        let ms = now
            .duration_since(self.presentation_clock)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let presentation = self
            .state
            .as_mut()
            .expect("notice state")
            .step(secs, ms, false);
        if presentation.mode != self.presentation.mode {
            self.resize(presentation.mode);
        }
        self.presentation = presentation;
        self.refresh();
    }
    fn resize(&mut self, mode: Mode) {
        unsafe {
            let dpi = GetDpiForWindow(self.hwnd) as i32;
            let u = |n: i32| n * dpi / 96;
            let monitor = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return;
            }
            let work = info.rcWork;
            let work_width = work.right - work.left;
            let layout = notice_layout(work_width, dpi);
            let (w, h, radius) = if self.badge {
                (u(190), u(48), u(20))
            } else {
                match mode {
                    Mode::Large => (layout.width, layout.height, layout.radius),
                    Mode::Capsule => (u(132), u(36), u(36)),
                }
            };
            let x = if self.badge {
                work.right - w - u(20)
            } else {
                work.left + u(20)
            };
            let y = work.top + u(20);
            let next = (x, y, w, h, radius, dpi, work_width);
            if self.geometry == Some(next) {
                return;
            }
            if self
                .geometry
                .is_none_or(|(_, _, old_w, old_h, old_r, old_dpi, _)| {
                    (old_w, old_h, old_r, old_dpi) != (w, h, radius, dpi)
                })
            {
                let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, radius, radius);
                if !region.is_null() && SetWindowRgn(self.hwnd, region, 0) == 0 {
                    DeleteObject(region);
                }
            }
            if self
                .geometry
                .is_none_or(|(old_x, old_y, old_w, old_h, _, _, _)| {
                    (old_x, old_y, old_w, old_h) != (x, y, w, h)
                })
            {
                SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    x,
                    y,
                    w,
                    h,
                    SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                );
            }
            self.geometry = Some(next);
            self.refresh();
        }
    }
    fn refresh(&mut self) {
        let Some((_, _, width, height, _, dpi, work_width)) = self.geometry else {
            return;
        };
        let next = RenderSnapshot {
            badge: self.badge,
            title: self.title.clone(),
            message: self.message.clone(),
            seconds: if self.badge {
                0
            } else {
                self.remaining_now(Instant::now())
            },
            mode: self.presentation.mode,
            fraction_bits: self.presentation.fraction.to_bits(),
            urgent: self.presentation.urgent,
            width,
            height,
            dpi,
            work_width,
        };
        if render_changed(self.last_render.as_ref(), &next) {
            self.last_render = Some(next);
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }
    }
    fn show(&self) -> Result<(), String> {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            UpdateWindow(self.hwnd);
            if IsWindowVisible(self.hwnd) == 0 {
                return Err("fenêtre invisible".into());
            }
        }
        Ok(())
    }
    fn paint(&self, dc: HDC, snapshot: &RenderSnapshot) {
        unsafe {
            let mut rect = std::mem::zeroed();
            GetClientRect(self.hwnd, &mut rect);
            let bg = CreateSolidBrush(0x00181a1c);
            FillRect(dc, &rect, bg);
            DeleteObject(bg);
            SetBkMode(dc, TRANSPARENT as i32);
            let dpi = snapshot.dpi;
            let u = |n: i32| n * dpi / 96;
            // Couleurs Kinseat (COLORREF = 0x00BBGGRR) : texte, secondaire, ambre, urgence.
            const PRIMARY: u32 = 0x00E1E8ED;
            const SECONDARY: u32 = 0x00A1ABB3;
            const ACCENT: u32 = 0x003DA3E8;
            const URGENT: u32 = 0x00606AEF;
            let layout = notice_layout(snapshot.work_width, dpi);
            let compact = |n: i32| n * layout.width / 300;
            let font = |size: i32, weight: i32| {
                CreateFontW(
                    -if self.badge || self.presentation.mode == Mode::Capsule {
                        u(size)
                    } else {
                        compact(size).max(u(12))
                    },
                    0,
                    0,
                    0,
                    weight,
                    0,
                    0,
                    0,
                    1,
                    0,
                    0,
                    5,
                    0,
                    wide(if weight >= 600 {
                        "Segoe UI Semibold"
                    } else {
                        "Segoe UI"
                    })
                    .as_ptr(),
                )
            };
            let draw =
                |text: &str, size: i32, weight: i32, color: u32, top: i32, bottom: i32, flags| {
                    let f = font(size, weight);
                    let old = SelectObject(dc, f);
                    SetTextColor(dc, color);
                    let mut r = RECT {
                        left: layout.margin,
                        top: compact(top),
                        right: rect.right - layout.margin,
                        bottom: compact(bottom),
                    };
                    let w = wide(text);
                    DrawTextW(dc, w.as_ptr(), -1, &mut r, flags);
                    SelectObject(dc, old);
                    DeleteObject(f);
                };
            if self.badge {
                // Pastille ambre + libellé : l'observation est visible pour le joueur.
                paint_ring(
                    dc,
                    u(17),
                    u(18),
                    u(13),
                    0.0,
                    u(11) as f64,
                    1.0,
                    [0x3d, 0xa3, 0xe8],
                    [0x3d, 0xa3, 0xe8],
                );
                let f = font(14, 600);
                let oldf = SelectObject(dc, f);
                SetTextColor(dc, PRIMARY);
                let mut r = RECT {
                    left: u(36),
                    top: 0,
                    right: rect.right - u(12),
                    bottom: rect.bottom,
                };
                let w = wide(&self.title);
                DrawTextW(
                    dc,
                    w.as_ptr(),
                    -1,
                    &mut r,
                    DT_LEFT | DT_SINGLELINE | DT_VCENTER,
                );
                SelectObject(dc, oldf);
                DeleteObject(f);
            } else if self.presentation.mode == Mode::Capsule {
                let color = if self.presentation.urgent {
                    URGENT
                } else {
                    ACCENT
                };
                let accent = if self.presentation.urgent {
                    [0x60, 0x6a, 0xef]
                } else {
                    [0x3d, 0xa3, 0xe8]
                };
                let size = u(24);
                paint_ring(
                    dc,
                    u(22) - size / 2,
                    u(18) - size / 2,
                    size,
                    u(9) as f64,
                    u(3) as f64,
                    self.presentation.fraction,
                    [0x3a, 0x36, 0x31],
                    accent,
                );
                let secs = snapshot.seconds;
                let f = font(13, 600);
                let old = SelectObject(dc, f);
                SetTextColor(dc, color);
                let mut r = RECT {
                    left: u(42),
                    top: 0,
                    right: rect.right - u(6),
                    bottom: rect.bottom,
                };
                let text = wide(&format!("{:02}:{:02}", secs / 60, secs % 60));
                DrawTextW(
                    dc,
                    text.as_ptr(),
                    -1,
                    &mut r,
                    DT_LEFT | DT_SINGLELINE | DT_VCENTER,
                );
                SelectObject(dc, old);
                DeleteObject(f);
            } else {
                let secs = snapshot.seconds;
                let tone = if urgent(secs) { URGENT } else { ACCENT };
                let bar = CreateSolidBrush(tone);
                let top_bar = RECT {
                    left: layout.margin,
                    top: 0,
                    right: rect.right - layout.margin,
                    bottom: compact(3),
                };
                FillRect(dc, &top_bar, bar);
                DeleteObject(bar);
                draw(
                    "FIN DE SESSION",
                    12,
                    600,
                    tone,
                    14,
                    34,
                    DT_LEFT | DT_SINGLELINE,
                );
                let countdown = format!("{:02}:{:02}", secs / 60, secs % 60);
                draw(
                    &countdown,
                    25,
                    600,
                    if urgent(secs) { URGENT } else { PRIMARY },
                    34,
                    68,
                    DT_LEFT | DT_SINGLELINE,
                );
                draw(
                    &self.message,
                    14,
                    400,
                    PRIMARY,
                    72,
                    116,
                    DT_LEFT | DT_WORDBREAK | DT_END_ELLIPSIS,
                );
                draw(
                    "À zéro, la session ferme sans sauvegarde.",
                    12,
                    400,
                    SECONDARY,
                    122,
                    156,
                    DT_LEFT | DT_WORDBREAK | DT_END_ELLIPSIS,
                );
            }
        }
    }
}
impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            DestroyWindow(self.hwnd);
        }
    }
}
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST {
        return HTTRANSPARENT as isize;
    }
    if msg == WM_ERASEBKGND {
        return 1;
    }
    if msg == WM_PAINT {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        if !dc.is_null() {
            let overlay = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Overlay;
            if let Some(overlay) = overlay.as_ref() {
                if let Some(snapshot) = overlay.last_render.as_ref() {
                    let memory = CreateCompatibleDC(dc);
                    if !memory.is_null() {
                        let bitmap = CreateCompatibleBitmap(dc, snapshot.width, snapshot.height);
                        if !bitmap.is_null() {
                            let old = SelectObject(memory, bitmap);
                            overlay.paint(memory, snapshot);
                            BitBlt(
                                dc,
                                0,
                                0,
                                snapshot.width,
                                snapshot.height,
                                memory,
                                0,
                                0,
                                SRCCOPY,
                            );
                            SelectObject(memory, old);
                            DeleteObject(bitmap);
                        }
                        DeleteDC(memory);
                    }
                }
            }
        }
        EndPaint(hwnd, &ps);
        return 0;
    }
    if msg == WM_DISPLAYCHANGE || msg == WM_DPICHANGED {
        // Reflow on the UI thread after Windows has applied the new DPI/display topology.
        PostMessageW(hwnd, WM_APP + 1, 0, 0);
    }
    DefWindowProcW(hwnd, msg, w, l)
}
fn pump(mut notice: Option<&mut Overlay>, mut badge: Option<&mut Overlay>) {
    if let Some(overlay) = notice.as_deref_mut() {
        overlay.tick();
    }
    unsafe {
        let mut msg = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if msg.message == WM_APP + 1 {
                if let Some(overlay) = notice.as_deref_mut() {
                    if overlay.hwnd == msg.hwnd {
                        overlay.resize(overlay.presentation.mode);
                    }
                }
                if let Some(overlay) = badge.as_deref_mut() {
                    if overlay.hwnd == msg.hwnd {
                        overlay.resize(Mode::Capsule);
                    }
                }
            }
        }
    }
}

// Captures primary desktop into a top-down DIB. Never persists pixels in normal operation.
fn capture(max_width: u32) -> Result<(Vec<u8>, u32, u32), String> {
    unsafe {
        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);
        if width <= 0 || height <= 0 || width > 16384 || height > 16384 {
            return Err("dimensions de l'écran invalides".into());
        }
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err(failure());
        }
        let memory = CreateCompatibleDC(screen);
        if memory.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err(failure());
        }
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits = std::ptr::null_mut();
        let bmp = CreateDIBSection(
            screen,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if bmp.is_null() {
            DeleteDC(memory);
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err(failure());
        }
        let old = SelectObject(memory, bmp);
        let ok = BitBlt(
            memory,
            0,
            0,
            width,
            height,
            screen,
            0,
            0,
            SRCCOPY | CAPTUREBLT,
        );
        let pixels = if ok != 0 && !bits.is_null() {
            Some(
                std::slice::from_raw_parts(bits as *const u8, width as usize * height as usize * 4)
                    .to_vec(),
            )
        } else {
            None
        };
        SelectObject(memory, old);
        DeleteObject(bmp);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
        let pixels = pixels.ok_or_else(failure)?;
        crate::image_processing::jpeg_from_bgra(&pixels, width as u32, height as u32, max_width)
    }
}
fn utc_now() -> String {
    // UTC ISO-8601 timestamp; the service determines freshness using its own clock.
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = t.as_secs() as i64;
    let days = secs / 86400;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        secs % 86400 / 3600,
        secs % 3600 / 60,
        secs % 60,
        t.subsec_millis()
    )
}
fn pipe_name(guid: &str) -> String {
    format!(r"\\.\pipe\Kinseat.Agent.v1.{guid}")
}
fn valid_guid(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

pub fn run(args: &[String]) -> Result<(), String> {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if args.len() == 2 && args[1] == "--selftest-notice-fast" {
        // 6m30s at 10x: first 5-minute crossing occurs after the initial 8s expansion.
        let mut overlay = Overlay::new(false)?;
        overlay.message = "Préavis accéléré — fermeture de la session".into();
        overlay.seconds = 390;
        overlay.speed = 10;
        overlay.since = Instant::now();
        overlay.state = Some(NoticeState::new(390, 0));
        overlay.show()?;
        while overlay.remaining_now(Instant::now()) > 0 {
            pump(Some(&mut overlay), None);
            std::thread::sleep(Duration::from_millis(30));
        }
        return Ok(());
    }
    if args.len() == 4 && args[1] == "--selftest-notice" {
        let seconds: u64 = args[2].parse().map_err(|_| "secondes invalides")?;
        let mut overlay = Overlay::new(false)?;
        overlay.message = args[3].clone();
        overlay.seconds = seconds;
        overlay.since = Instant::now();
        overlay.state = Some(NoticeState::new(seconds, 0));
        overlay.show()?;
        while overlay.remaining_now(Instant::now()) > 0 {
            pump(Some(&mut overlay), None);
            std::thread::sleep(Duration::from_millis(100));
        }
        return Ok(());
    }
    if args.len() == 3 && args[1] == "--selftest-capture" {
        let (jpeg, _, _) = capture(1280)?;
        std::fs::write(&args[2], jpeg).map_err(|e| e.to_string())?;
        return Ok(());
    }
    if args.len() != 5
        || args[1] != "--seat"
        || args[3] != "--pipe"
        || !valid_guid(&args[2])
        || args[4] != pipe_name(&args[2])
    {
        return Err("usage : --seat <guid> --pipe \\\\.\\pipe\\Kinseat.Agent.v1.<guid>".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&args[4])
        .map_err(|e| e.to_string())?;
    let reader = file.try_clone().map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Sync I/O on one pipe file object is serialised by Windows: a ReadFile pending
        // here would block every WriteFile of the main loop (frames, notice.displayed).
        let mut reader = BufReader::new(PeekReader(reader));
        loop {
            match protocol::read_line(&mut reader) {
                Ok(Some(msg)) => {
                    if tx.send(Some(msg)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    log("pipe read: EOF");
                    let _ = tx.send(None);
                    break;
                }
                Err(e) => {
                    log(&format!("pipe read error: {e}"));
                    let _ = tx.send(None);
                    break;
                }
            }
        }
    });
    let mut writer = BufWriter::new(file);
    let mut session = 0;
    unsafe {
        ProcessIdToSessionId(GetCurrentProcessId(), &mut session);
    }
    protocol::write_line(
        &mut writer,
        &Outgoing::Hello {
            session_id: session,
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION"),
        },
    )
    .map_err(|e| {
        log(&format!("pipe hello write error: {e}"));
        e.to_string()
    })?;
    log(&format!(
        "hello session={session} pid={}",
        std::process::id()
    ));
    let mut notice: Option<(String, Box<Overlay>)> = None;
    let mut badge: Option<Box<Overlay>> = None;
    let mut preview: Option<(u32, Duration, Instant)> = None;
    let mut frames = 0_u64;
    loop {
        loop {
            let msg = match rx.try_recv() {
                Ok(msg) => msg,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("pipe reader thread stopped".into());
                }
            };
            let Some(msg) = msg else {
                log("exit: pipe disconnected");
                return Ok(());
            };
            log(match &msg {
                Incoming::NoticeShow { .. } => "recv notice.show",
                Incoming::NoticeHide { .. } => "recv notice.hide",
                Incoming::PreviewStart { .. } => "recv preview.start",
                Incoming::PreviewStop => "recv preview.stop",
            });
            match msg {
                Incoming::NoticeShow {
                    notice_id,
                    message,
                    remaining_seconds,
                } => {
                    if message.chars().count() > 220 {
                        protocol::write_line(
                            &mut writer,
                            &Outgoing::NoticeFailed {
                                notice_id: &notice_id,
                                reason: "message trop long",
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        continue;
                    }
                    let result = (|| {
                        let same_notice = notice.as_ref().is_some_and(|(id, _)| *id == notice_id);
                        let mut o = notice
                            .take()
                            .map(|(_, o)| o)
                            .map(Ok)
                            .unwrap_or_else(|| Overlay::new(false))?;
                        let changed = o.message != message;
                        o.message = message;
                        o.seconds = remaining_seconds;
                        o.since = Instant::now();
                        if !same_notice {
                            o.presentation_clock = o.since;
                            o.state = Some(NoticeState::new(remaining_seconds, 0));
                            if o.presentation.mode == Mode::Capsule {
                                o.resize(Mode::Large);
                            }
                            o.presentation.mode = Mode::Large;
                        } else {
                            let ms =
                                o.since.duration_since(o.presentation_clock).as_millis() as u64;
                            let presentation = o.state.as_mut().expect("notice state").step(
                                remaining_seconds,
                                ms,
                                changed,
                            );
                            if presentation.mode != o.presentation.mode {
                                o.resize(presentation.mode);
                            }
                            o.presentation = presentation;
                        }
                        o.refresh();
                        o.show()?;
                        pump(Some(&mut o), badge.as_deref_mut());
                        Ok::<_, String>(o)
                    })();
                    match result {
                        Ok(o) => {
                            notice = Some((notice_id.clone(), o));
                            log("notice.displayed");
                            protocol::write_line(
                                &mut writer,
                                &Outgoing::NoticeDisplayed {
                                    notice_id: &notice_id,
                                },
                            )
                        }
                        Err(reason) => {
                            log(&format!("notice.failed: {reason}"));
                            protocol::write_line(
                                &mut writer,
                                &Outgoing::NoticeFailed {
                                    notice_id: &notice_id,
                                    reason: &reason,
                                },
                            )
                        }
                    }
                    .map_err(|e| e.to_string())?;
                }
                Incoming::NoticeHide { notice_id } => {
                    if notice.as_ref().is_some_and(|(id, _)| *id == notice_id) {
                        notice = None;
                    }
                }
                Incoming::PreviewStart {
                    max_width,
                    fps,
                    show_indicator,
                } => {
                    if max_width == 0 || max_width > 1280 || !(1..=4).contains(&fps) {
                        protocol::write_line(
                            &mut writer,
                            &Outgoing::PreviewFailed {
                                reason: "paramètres de capture invalides",
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        continue;
                    }
                    badge = None;
                    if show_indicator {
                        match Overlay::new(true).and_then(|b| {
                            b.show()?;
                            Ok(b)
                        }) {
                            Ok(b) => badge = Some(b),
                            Err(reason) => {
                                protocol::write_line(
                                    &mut writer,
                                    &Outgoing::PreviewFailed { reason: &reason },
                                )
                                .map_err(|e| e.to_string())?;
                                continue;
                            }
                        }
                    }
                    frames = 0;
                    log("preview started");
                    preview = Some((
                        max_width,
                        Duration::from_millis(1000 / fps as u64),
                        Instant::now(),
                    ));
                }
                Incoming::PreviewStop => {
                    log(&format!("preview stopped frames={frames}"));
                    preview = None;
                    badge = None;
                }
            }
        }
        pump(notice.as_mut().map(|(_, o)| &mut **o), badge.as_deref_mut());
        if let Some((width, interval, next)) = &mut preview {
            if Instant::now() >= *next {
                *next = Instant::now() + *interval;
                let started = Instant::now();
                match capture(*width) {
                    Ok((jpeg, w, h)) => {
                        let jpeg_size = jpeg.len();
                        let encoded = base64::engine::general_purpose::STANDARD.encode(jpeg);
                        if encoded.len() + 256 > protocol::MAX_LINE {
                            log(&format!(
                                "preview frame skipped: encoded size={}",
                                encoded.len()
                            ));
                            continue;
                        }
                        let stamp = utc_now();
                        protocol::write_line(
                            &mut writer,
                            &Outgoing::PreviewFrame {
                                jpeg_base64: &encoded,
                                width: w,
                                height: h,
                                captured_at_utc: &stamp,
                            },
                        )
                        .map_err(|e| format!("preview frame pipe write: {e}"))?;
                        frames += 1;
                        log(&format!("preview frame={frames} jpeg_bytes={jpeg_size} dimensions={w}x{h} capture_ms={}", started.elapsed().as_millis()));
                    }
                    Err(reason) => {
                        // GDI may fail temporarily while a layered overlay or display topology changes.
                        // The notice and pipe must remain alive; retry on the next frame.
                        log(&format!("preview capture skipped: {reason}"));
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}
