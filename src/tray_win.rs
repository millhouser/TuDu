//! Windows System-Tray via winapi.
//!
//! Läuft in einem eigenen Background-Thread mit eigener Win32-Message-Pump.
//! Events werden per mpsc::channel an den Slint-Hauptthread geschickt.

use std::sync::mpsc;

#[derive(Debug)]
pub enum TrayEvent {
    Show,
    Quit,
}

/// Startet den Tray-Thread und gibt einen Receiver zurück.
pub fn spawn(icon_rgba: Vec<u8>, icon_w: u32, icon_h: u32) -> mpsc::Receiver<TrayEvent> {
    let (tx, rx) = mpsc::channel::<TrayEvent>();
    std::thread::spawn(move || run(tx, icon_rgba, icon_w, icon_h));
    rx
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use super::TrayEvent;
    use std::{ptr, sync::mpsc};

    use winapi::shared::windef::{HICON, HMENU, HWND};
    use winapi::shared::minwindef::UINT;
    use winapi::um::libloaderapi::GetModuleHandleW;
    use winapi::um::shellapi::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP,
        NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    };
    use winapi::um::winuser::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
        DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos,
        GetMessageW, MSG, MF_STRING, PostMessageW, PostQuitMessage,
        RegisterClassW, SetForegroundWindow, TrackPopupMenu, TranslateMessage,
        HWND_MESSAGE, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
        WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_LBUTTONDBLCLK,
        WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
    };

    const WM_TRAYICON: UINT = WM_APP + 1;
    const ID_TRAY:     UINT = 1;
    const IDM_OPEN:    usize = 100;
    const IDM_QUIT:    usize = 101;

    thread_local! {
        static TX_PTR:    std::cell::RefCell<usize> = std::cell::RefCell::new(0);
        static HMENU_PTR: std::cell::RefCell<usize> = std::cell::RefCell::new(0);
    }

    pub fn run(tx: mpsc::Sender<TrayEvent>, icon_rgba: Vec<u8>, icon_w: u32, icon_h: u32) {
        unsafe {
            let class_name: Vec<u16> = "teuxdeux_tray\0".encode_utf16().collect();
            let hinstance = GetModuleHandleW(ptr::null());

            TX_PTR.with(|c| *c.borrow_mut() = Box::into_raw(Box::new(tx)) as usize);

            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: ptr::null_mut(),
                hCursor: ptr::null_mut(),
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };
            RegisterClassW(&wc);

            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                "teuxdeux_tray\0".encode_utf16().collect::<Vec<_>>().as_ptr(),
                0, 0, 0, 0, 0,
                HWND_MESSAGE, ptr::null_mut(), hinstance, ptr::null_mut(),
            );
            if hwnd.is_null() { return; }

            let mut bgra = icon_rgba;
            for chunk in bgra.chunks_exact_mut(4) { chunk.swap(0, 2); }
            let hicon = make_hicon(&bgra, icon_w, icon_h).unwrap_or(ptr::null_mut());

            let mut tip: [u16; 128] = [0; 128];
            for (i, ch) in "teuxdeux\0".encode_utf16().enumerate().take(127) {
                tip[i] = ch;
            }
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize           = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd             = hwnd;
            nid.uID              = ID_TRAY;
            nid.uFlags           = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = WM_TRAYICON;
            nid.hIcon            = hicon;
            nid.szTip            = tip;
            Shell_NotifyIconW(NIM_ADD, &mut nid);

            let hmenu = CreatePopupMenu();
            let open_w: Vec<u16> = "\u{00d6}ffnen\0".encode_utf16().collect();
            let quit_w: Vec<u16> = "Beenden\0".encode_utf16().collect();
            AppendMenuW(hmenu, MF_STRING, IDM_OPEN, open_w.as_ptr());
            AppendMenuW(hmenu, MF_STRING, IDM_QUIT, quit_w.as_ptr());
            HMENU_PTR.with(|c| *c.borrow_mut() = hmenu as usize);

            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            Shell_NotifyIconW(NIM_DELETE, &mut nid);
            DestroyWindow(hwnd);
            DestroyMenu(hmenu);
            if !hicon.is_null() {
                winapi::um::winuser::DestroyIcon(hicon);
            }
        }
    }

    unsafe fn make_hicon(bgra: &[u8], w: u32, h: u32) -> Option<HICON> {
        use winapi::um::wingdi::{
            BI_BITFIELDS, BITMAPINFO, BITMAPV5HEADER, CreateBitmap,
            CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject,
            DIB_RGB_COLORS,
        };
        use winapi::um::winuser::{CreateIconIndirect, ICONINFO};

        let hdc = CreateCompatibleDC(ptr::null_mut());
        if hdc.is_null() { return None; }

        let mut bi: BITMAPV5HEADER = std::mem::zeroed();
        bi.bV5Size        = std::mem::size_of::<BITMAPV5HEADER>() as u32;
        bi.bV5Width       = w as i32;
        bi.bV5Height      = -(h as i32);
        bi.bV5Planes      = 1;
        bi.bV5BitCount    = 32;
        bi.bV5Compression = BI_BITFIELDS;
        bi.bV5RedMask     = 0x00FF_0000;
        bi.bV5GreenMask   = 0x0000_FF00;
        bi.bV5BlueMask    = 0x0000_00FF;
        bi.bV5AlphaMask   = 0xFF00_0000u32;

        let mut bits: *mut winapi::ctypes::c_void = ptr::null_mut();
        let bits_ptr: *mut *mut winapi::ctypes::c_void = &mut bits;
        let hbm_color = CreateDIBSection(
            hdc,
            &bi as *const _ as *const BITMAPINFO,
            DIB_RGB_COLORS,
            bits_ptr,
            ptr::null_mut(),
            0,
        );
        if hbm_color.is_null() { DeleteDC(hdc); return None; }
        ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());

        let hbm_mask = CreateBitmap(w as i32, h as i32, 1, 1, ptr::null());
        let ii = ICONINFO {
            fIcon: 1, xHotspot: 0, yHotspot: 0,
            hbmMask: hbm_mask, hbmColor: hbm_color,
        };
        let hicon = CreateIconIndirect(&ii as *const _ as *mut _);
        DeleteObject(hbm_color as *mut _);
        DeleteObject(hbm_mask  as *mut _);
        DeleteDC(hdc);
        if hicon.is_null() { None } else { Some(hicon) }
    }

    fn send_event(ev: TrayEvent) {
        TX_PTR.with(|cell| {
            let ptr = *cell.borrow();
            if ptr != 0 {
                let tx = unsafe { &*(ptr as *const mpsc::Sender<TrayEvent>) };
                let _ = tx.send(ev);
            }
        });
    }

    pub unsafe extern "system" fn wnd_proc(
        hwnd:   HWND,
        msg:    UINT,
        wparam: winapi::shared::minwindef::WPARAM,
        lparam: winapi::shared::minwindef::LPARAM,
    ) -> winapi::shared::minwindef::LRESULT {
        if msg == WM_TRAYICON {
            let event = (lparam & 0xFFFF) as UINT;
            match event {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                    send_event(TrayEvent::Show);
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    let mut pt: winapi::shared::windef::POINT = std::mem::zeroed();
                    GetCursorPos(&mut pt);
                    SetForegroundWindow(hwnd);
                    let hmenu = HMENU_PTR.with(|c| *c.borrow()) as HMENU;
                    TrackPopupMenu(
                        hmenu, TPM_BOTTOMALIGN | TPM_LEFTALIGN,
                        pt.x, pt.y, 0, hwnd, ptr::null(),
                    );
                    PostMessageW(hwnd, WM_NULL, 0, 0);
                }
                _ => {}
            }
        } else if msg == WM_COMMAND {
            let cmd = (wparam & 0xFFFF) as usize;
            if cmd == IDM_OPEN {
                send_event(TrayEvent::Show);
            } else if cmd == IDM_QUIT {
                send_event(TrayEvent::Quit);
                PostQuitMessage(0);
            }
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

// ─── Öffentliche run-Funktion ─────────────────────────────────────────────────

#[cfg(windows)]
pub fn run(tx: mpsc::Sender<TrayEvent>, rgba: Vec<u8>, w: u32, h: u32) {
    imp::run(tx, rgba, w, h);
}

#[cfg(not(windows))]
pub fn run(_tx: mpsc::Sender<TrayEvent>, _rgba: Vec<u8>, _w: u32, _h: u32) {}
