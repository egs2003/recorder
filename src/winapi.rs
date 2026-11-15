use std::ffi::c_void;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsIconic,
    IsWindowVisible, SetForegroundWindow, ShowWindow, GWL_EXSTYLE, SW_RESTORE, WS_EX_TOOLWINDOW,
};

#[derive(Clone, Debug)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub primary: bool,
}

impl Monitor {
    pub fn label(&self, idx: usize) -> String {
        format!(
            "Monitor {} - {}x{} bei ({}, {}){}",
            idx + 1,
            self.w,
            self.h,
            self.x,
            self.y,
            if self.primary { " - primär" } else { "" }
        )
    }
}

/// Muss vor dem Erzeugen des Fensters aufgerufen werden, damit alle Koordinaten
/// echte Pixel sind.
pub fn set_dpi_aware() {
    unsafe {
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4
        SetProcessDpiAwarenessContext(-4);
    }
}

unsafe extern "system" fn monitor_cb(h: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
    let v = &mut *(lp as *mut Vec<Monitor>);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(h, &mut mi) != 0 {
        let r = mi.rcMonitor;
        v.push(Monitor {
            x: r.left,
            y: r.top,
            w: r.right - r.left,
            h: r.bottom - r.top,
            primary: mi.dwFlags & 1 != 0,
        });
    }
    1
}

pub fn list_monitors() -> Vec<Monitor> {
    let mut v: Vec<Monitor> = Vec::new();
    unsafe {
        EnumDisplayMonitors(0, std::ptr::null(), Some(monitor_cb), &mut v as *mut _ as LPARAM);
    }
    // Primärer Monitor zuerst, danach von links nach rechts
    v.sort_by_key(|m| (!m.primary, m.x, m.y));
    v
}

/// (x, y, Breite, Höhe) des gesamten virtuellen Desktops
pub fn virtual_bounds(m: &[Monitor]) -> (i32, i32, i32, i32) {
    if m.is_empty() {
        return (0, 0, 1920, 1080);
    }
    let x0 = m.iter().map(|m| m.x).min().unwrap();
    let y0 = m.iter().map(|m| m.y).min().unwrap();
    let x1 = m.iter().map(|m| m.x + m.w).max().unwrap();
    let y1 = m.iter().map(|m| m.y + m.h).max().unwrap();
    (x0, y0, x1 - x0, y1 - y0)
}

unsafe extern "system" fn window_cb(h: HWND, lp: LPARAM) -> BOOL {
    let v = &mut *(lp as *mut Vec<(isize, String)>);
    if IsWindowVisible(h) == 0 || IsIconic(h) != 0 {
        return 1;
    }
    let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW != 0 {
        return 1;
    }
    let mut cloaked: u32 = 0;
    // 14 = DWMWA_CLOAKED (z.B. Hintergrund-UWP-Fenster)
    DwmGetWindowAttribute(h, 14, &mut cloaked as *mut u32 as *mut c_void, 4);
    if cloaked != 0 {
        return 1;
    }
    let len = GetWindowTextLengthW(h);
    if len <= 0 {
        return 1;
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
    if n > 0 {
        v.push((h, String::from_utf16_lossy(&buf[..n as usize])));
    }
    1
}

fn enum_windows() -> Vec<(isize, String)> {
    let mut v: Vec<(isize, String)> = Vec::new();
    unsafe {
        EnumWindows(Some(window_cb), &mut v as *mut _ as LPARAM);
    }
    v.retain(|(_, t)| t != "Program Manager" && t != crate::APP_TITLE);
    v
}

pub fn list_windows() -> Vec<String> {
    let mut v: Vec<String> = enum_windows().into_iter().map(|(_, t)| t).collect();
    v.sort_by_key(|t| t.to_lowercase());
    v.dedup();
    v
}

/// Findet ein sichtbares Fenster anhand seines Titels.
pub fn find_window(title: &str) -> Option<isize> {
    enum_windows().into_iter().find(|(_, t)| t == title).map(|(h, _)| h)
}

pub fn bring_to_front(hwnd: isize) {
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
}

/// Sichtbarer Fensterbereich (ohne unsichtbaren Schattenrand) in Bildschirmpixeln: (x, y, w, h)
pub fn window_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        // 9 = DWMWA_EXTENDED_FRAME_BOUNDS
        let hr = DwmGetWindowAttribute(hwnd, 9, &mut r as *mut RECT as *mut c_void, std::mem::size_of::<RECT>() as u32);
        if hr != 0 && GetWindowRect(hwnd, &mut r) == 0 {
            return None;
        }
        Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
    }
}
