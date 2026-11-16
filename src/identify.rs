//! Monitor-Identifizierung: blendet auf jedem Monitor kurz eine große Nummer ein (wie in den
//! Windows-Anzeigeeinstellungen). Der gerade gewählte Monitor ist blau, die anderen dunkel.
//! Die Fenster sind klick-durchlässig, nehmen keinen Fokus und verschwinden von selbst wieder.
//! Die Nummer entspricht der Nummer in der Monitor-Auswahlliste.

use std::cell::{Cell, RefCell};
use std::sync::Mutex;
use std::thread;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, EndPaint, FillRect, SetWindowRgn, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW, PostMessageW,
    PostQuitMessage, RegisterClassW, SetLayeredWindowAttributes, SetTimer, ShowWindow,
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
    /// Nummer (ab 1) je Fenster dieses Threads
    static NUMBERS: RefCell<Vec<(isize, usize)>> = const { RefCell::new(Vec::new()) };
}

/// Welche der sieben Segmente (a oben, b oben rechts, c unten rechts, d unten, e unten links,
/// f oben links, g Mitte) für die Ziffer leuchten. Bit 0 = a ... Bit 6 = g.
const SEGMENTS: [u8; 10] = [
    0b0111111, // 0: a b c d e f
    0b0000110, // 1: b c
    0b1011011, // 2: a b d e g
    0b1001111, // 3: a b c d g
    0b1100110, // 4: b c f g
    0b1101101, // 5: a c d f g
    0b1111101, // 6: a c d e f g
    0b0000111, // 7: a b c
    0b1111111, // 8: alle
    0b1101111, // 9: a b c d f g
];

/// Zeichnet eine Ziffer als Segmentanzeige aus gefüllten Rechtecken (keine Schriftart nötig).
unsafe fn draw_digit(hdc: isize, brush: isize, digit: usize, x: i32, y: i32, w: i32, h: i32) {
    let t = (w / 5).max(3);
    let mid = (h - t) / 2;
    // (links, oben, rechts, unten) je Segment a..g
    let rects = [
        (x, y, x + w, y + t),
        (x + w - t, y, x + w, y + mid + t),
        (x + w - t, y + mid, x + w, y + h),
        (x, y + h - t, x + w, y + h),
        (x, y + mid, x + t, y + h),
        (x, y, x + t, y + mid + t),
        (x, y + mid, x + w, y + mid + t),
    ];
    let mask = SEGMENTS[digit % 10];
    for (i, &(l, tp, r, b)) in rects.iter().enumerate() {
        if mask & (1 << i) != 0 {
            let rc = RECT { left: l, top: tp, right: r, bottom: b };
            FillRect(hdc, &rc, brush);
        }
    }
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
    let bg = CreateSolidBrush(if selected { COL_SELECTED } else { COL_OTHER });
    FillRect(hdc, &rc, bg);
    DeleteObject(bg);

    let number = NUMBERS.with(|n| n.borrow().iter().find(|(hw, _)| *hw == h).map(|&(_, v)| v)).unwrap_or(0);
    let digits: Vec<usize> = number.to_string().bytes().map(|b| (b - b'0') as usize).collect();
    let dh = rc.bottom * 55 / 100;
    let dw = dh * 55 / 100;
    let gap = dw / 3;
    let total = digits.len() as i32 * dw + (digits.len() as i32 - 1) * gap;
    let mut x = (rc.right - total) / 2;
    let y = (rc.bottom - dh) / 2;
    let fg = CreateSolidBrush(COL_TEXT);
    for d in digits {
        draw_digit(hdc, fg, d, x, y, dw, dh);
        x += dw + gap;
    }
    DeleteObject(fg);
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
            NUMBERS.with(|n| n.borrow_mut().push((hwnd, i + 1)));
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
