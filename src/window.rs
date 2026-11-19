use std::{io, num::NonZeroIsize, ptr, time::Instant};

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{BLACK_BRUSH, GetStockObject},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::{
            AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem,
            GetDpiForWindow, SetProcessDpiAwarenessContext,
        },
        WindowsAndMessaging::{
            CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
            GetMessageW, IDC_ARROW, KillTimer, LoadCursorW, LoadIconW, MINMAXINFO, MSG,
            PostQuitMessage, RegisterClassW, SW_SHOW, SetTimer, ShowWindow, TranslateMessage,
            WM_APP, WM_CLOSE, WM_GETMINMAXINFO, WM_TIMER, WNDCLASSW, WS_CLIPCHILDREN,
            WS_OVERLAPPEDWINDOW,
        },
    },
};

use crate::platform::wide;

pub const APP_EVENT: u32 = WM_APP + 1;
const ICON_RESOURCE: u16 = 1;
const CONNECTION_TIMER: usize = 1;
const STYLE: u32 = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;

pub enum NativeMessage {
    Events,
    Timer,
    Other,
    Quit,
}

pub struct Window(HWND);

impl Window {
    pub fn new(title: &str) -> io::Result<Self> {
        let class_name = wide("MaklyxaTube.Window");
        let title = wide(title);
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let instance = GetModuleHandleW(ptr::null());
            if instance.is_null() {
                return Err(io::Error::last_os_error());
            }
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                hIcon: LoadIconW(instance, ptr::without_provenance(ICON_RESOURCE as usize)),
                hbrBackground: GetStockObject(BLACK_BRUSH),
                lpszClassName: class_name.as_ptr(),
                ..WNDCLASSW::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(io::Error::last_os_error());
            }
            let (width, height) = outer_size(1380, 860, GetDpiForSystem());
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                STYLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                width,
                height,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
            if hwnd.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(hwnd))
        }
    }

    pub fn handle(&self) -> HWND {
        self.0
    }

    pub fn show(&self) {
        unsafe { ShowWindow(self.0, SW_SHOW) };
    }

    pub fn schedule(&self, deadline: Option<Instant>) -> io::Result<()> {
        unsafe {
            if let Some(at) = deadline {
                let ms = at
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(u128::from(u32::MAX - 1)) as u32
                    + 1;
                if SetTimer(self.0, CONNECTION_TIMER, ms, None) == 0 {
                    return Err(io::Error::last_os_error());
                }
            } else {
                KillTimer(self.0, CONNECTION_TIMER);
            }
        }
        Ok(())
    }

    pub fn next_message(&self) -> io::Result<NativeMessage> {
        let mut message = MSG::default();
        let received = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
        match received {
            -1 => return Err(io::Error::last_os_error()),
            0 => return Ok(NativeMessage::Quit),
            _ => {}
        }
        if message.hwnd == self.0 {
            match message.message {
                APP_EVENT => return Ok(NativeMessage::Events),
                WM_TIMER if message.wParam == CONNECTION_TIMER => return Ok(NativeMessage::Timer),
                _ => {}
            }
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        Ok(NativeMessage::Other)
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0 as isize).ok_or(HandleError::Unavailable)?;
        let handle = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        Ok(unsafe { WindowHandle::borrow_raw(handle) })
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) };
    }
}

fn outer_size(width: i32, height: i32, dpi: u32) -> (i32, i32) {
    let dpi = if dpi == 0 { 96 } else { dpi };
    let mut rect = RECT {
        right: width * dpi as i32 / 96,
        bottom: height * dpi as i32 / 96,
        ..RECT::default()
    };
    unsafe { AdjustWindowRectExForDpi(&mut rect, STYLE, 0, 0, dpi) };
    (rect.right - rect.left, rect.bottom - rect.top)
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CLOSE => {
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_GETMINMAXINFO => {
            let info = unsafe { &mut *(lparam as *mut MINMAXINFO) };
            let (width, height) = outer_size(900, 560, unsafe { GetDpiForWindow(window) });
            info.ptMinTrackSize.x = width;
            info.ptMinTrackSize.y = height;
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
