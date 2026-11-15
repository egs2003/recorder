//! Roter Rahmen um den Aufnahmebereich. Das Fenster ist klick-durchlässig und per
//! WDA_EXCLUDEFROMCAPTURE von der Bildschirmaufnahme ausgenommen (Windows 10, Version 2004+).
//! Wird die Ausnahme nicht unterstützt, wird gar kein Rahmen angezeigt, damit er nie im Video landet.

use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, CreateSolidBrush, DeleteObject, SetWindowRgn, RGN_DIFF};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW, PostQuitMessage,
    RegisterClassW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, ShowWindow, TranslateMessage, HTTRANSPARENT,
    LWA_ALPHA, MSG, SW_SHOWNOACTIVATE, WM_CLOSE, WM_DESTROY, WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const THICKNESS: i32 = 4;

pub struct Indicator {
    hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        _ => DefWindowProcW(h, m, w, l),
    }
}

impl Indicator {
    pub fn show(x: i32, y: i32, w: i32, h: i32) -> Option<Indicator> {
        let (tx, rx) = mpsc::channel::<isize>();
        let thread = thread::spawn(move || unsafe {
            let class: Vec<u16> = "RecorderRecIndicator\0".encode_utf16().collect();
            let hinst = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: 0,
                hCursor: 0,
                hbrBackground: CreateSolidBrush(0x0000_00FF), // Rot (BGR)
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc); // schlägt beim zweiten Mal fehl, das ist egal
            let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            let hwnd = CreateWindowExW(
                ex,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                x,
                y,
                w,
                h,
                0,
                0,
                hinst,
                std::ptr::null(),
            );
            if hwnd == 0 {
                let _ = tx.send(0);
                return;
            }
            // 0x11 = WDA_EXCLUDEFROMCAPTURE
            if SetWindowDisplayAffinity(hwnd, 0x11) == 0 {
                DestroyWindow(hwnd);
                let _ = tx.send(0);
                return;
            }
            SetLayeredWindowAttributes(hwnd, 0, 230, LWA_ALPHA);
            // Nur ein Rahmen: außen minus innen
            let outer = CreateRectRgn(0, 0, w, h);
            let inner = CreateRectRgn(THICKNESS, THICKNESS, w - THICKNESS, h - THICKNESS);
            CombineRgn(outer, outer, inner, RGN_DIFF);
            DeleteObject(inner);
            SetWindowRgn(hwnd, outer, 1);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = tx.send(hwnd);

            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, 0, 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(h) if h != 0 => Some(Indicator { hwnd: h, thread: Some(thread) }),
            _ => None,
        }
    }
}

impl Drop for Indicator {
    fn drop(&mut self) {
        unsafe {
            PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
