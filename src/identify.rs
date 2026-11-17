//! Monitor-Identifizierung: blendet auf jedem Monitor kurz eine große Nummer ein (wie in den
//! Windows-Anzeigeeinstellungen). Der gerade gewählte Monitor ist blau, die anderen dunkel.
//! Die Fenster sind klick-durchlässig, nehmen keinen Fokus und verschwinden von selbst wieder.
//! Die Nummer entspricht der Nummer in der Monitor-Auswahlliste.

use std::cell::Cell;
use std::sync::Mutex;
use std::thread;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, EndPaint, FillRect, SelectObject,
    SetBkMode, SetTextColor, SetWindowRgn, DrawTextW, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW, GetWindowTextW,
    PostMessageW, PostQuitMessage, RegisterClassW, SetLayeredWindowAttributes, SetTimer, ShowWindow,
    TranslateMessage, HTTRANSPARENT, LWA_ALPHA, MSG, SW_SHOWNOACTIVATE, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND,
    WM_NCHITTEST, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

/// So lange bleibt die Einblendung sichtbar.
const SHOW_MS: u32 = 2500;
/// COLORREF ist 0x00BBGGRR
const COL_SELECTED: u32 = 0x00FF_8456; // RGB(86, 132, 255)
const COL_OTHER: u32 = 0x0028_211E; // RGB(30, 33, 40)
const COL_TEXT: u32 = 0x00FF_FFFF;

/// Fenster der aktuell sichtbaren Einblendung (damit eine neue die alte ersetzt).
static OPEN: Mutex<Vec<isize>> = Mutex::new(Vec::new());

thread_local! {
    /// Anzahl noch offener Fenster dieses Threads; bei 0 endet die Nachrichtenschleife.
    static REMAINING: Cell<usize> = const { Cell::new(0) };
    /// Fenster des gewählten Monitors (wird blau gezeichnet).
    static SELECTED: Cell<isize> = const { Cell::new(0) };
}

fn forget(h: isize) {
    if let Ok(mut v) = OPEN.lock() {
        v.retain(|&x| x != h);
    }
}

unsafe fn paint(h: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(h, &mut ps);
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(h, &mut rc);

    let selected = SELECTED.with(|s| s.get()) == h;
    let brush = CreateSolidBrush(if selected { COL_SELECTED } else { COL_OTHER });
    FillRect(hdc, &rc, brush);
    DeleteObject(brush);

    let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
    // Höhe negativ = Zeichenhöhe in Pixeln; 700 = fett; 1 = DEFAULT_CHARSET; 4 = ANTIALIASED_QUALITY
    let font = CreateFontW(-(rc.bottom * 7 / 10), 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 4, 0, face.as_ptr());
    let old = SelectObject(hdc, font);
    SetBkMode(hdc, 1); // TRANSPARENT
    SetTextColor(hdc, COL_TEXT);

    let mut buf = [0u16; 16];
    let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
    // 0x25 = DT_CENTER | DT_VCENTER | DT_SINGLELINE
    DrawTextW(hdc, buf.as_ptr(), n, &mut rc, 0x25);

    SelectObject(hdc, old);
    DeleteObject(font);
    EndPaint(h, &ps);
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_PAINT => {
            paint(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_TIMER => {
            DestroyWindow(h);
            0
        }
        WM_DESTROY => {
            forget(h);
            let left = REMAINING.with(|r| {
                r.set(r.get().saturating_sub(1));
                r.get()
            });
            if left == 0 {
                PostQuitMessage(0);
            }
            0
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

/// Zeigt auf jedem Monitor seine Nummer (Listenposition + 1). `monitors` sind (x, y, Breite, Höhe)
/// in Bildschirmpixeln, `selected` der Index des gewählten Monitors. Kehrt sofort zurück.
pub fn show(monitors: &[(i32, i32, i32, i32)], selected: usize) {
    if monitors.is_empty() {
        return;
    }
    // vorherige Einblendung sofort beenden
    if let Ok(v) = OPEN.lock() {
        for &h in v.iter() {
            unsafe {
                PostMessageW(h, WM_CLOSE, 0, 0);
            }
        }
    }
    let monitors = monitors.to_vec();
    thread::spawn(move || unsafe {
        let class: Vec<u16> = "RecorderMonitorId\0".encode_utf16().collect();
        let hinst = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc); // schlägt beim zweiten Mal fehl, das ist egal
        let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;

        for (i, &(mx, my, mw, mh)) in monitors.iter().enumerate() {
            let size = (mh / 5).clamp(140, 360);
            let title: Vec<u16> = format!("{}\0", i + 1).encode_utf16().collect();
            let hwnd = CreateWindowExW(
                ex,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                mx + (mw - size) / 2,
                my + (mh - size) / 2,
                size,
                size,
                0,
                0,
                hinst,
                std::ptr::null(),
            );
            if hwnd == 0 {
                continue;
            }
            if i == selected {
                SELECTED.with(|s| s.set(hwnd));
            }
            SetLayeredWindowAttributes(hwnd, 0, 235, LWA_ALPHA);
            SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, size + 1, size + 1, size / 6, size / 6), 1);
            SetTimer(hwnd, 1, SHOW_MS, None);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            REMAINING.with(|r| r.set(r.get() + 1));
            if let Ok(mut v) = OPEN.lock() {
                v.push(hwnd);
            }
        }

        let mut msg: MSG = std::mem::zeroed();
        while REMAINING.with(|r| r.get()) > 0 && GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}
