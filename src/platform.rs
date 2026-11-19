use std::{
    env,
    ffi::{OsStr, c_void},
    fs::{self, OpenOptions},
    io::{self, Write},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr, thread,
    time::Duration,
};

use directories::{BaseDirs, UserDirs};
use tempfile::{NamedTempFile, TempDir};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM},
    System::Threading::CreateMutexW,
    UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, IsWindowVisible, MB_ICONERROR, MB_OK, MB_TOPMOST, MessageBoxW,
        SW_RESTORE, SetForegroundWindow, ShowWindow,
    },
};

use crate::model::Settings;

pub const APP_NAME: &str = "MaklyxaTube";
const MUTEX_NAME: &str = r"Local\MaklyxaTube.SingleInstance";

#[derive(Clone)]
pub struct Paths {
    settings: PathBuf,
    pub log: PathBuf,
    pub webview: PathBuf,
    legacy_webview: PathBuf,
    pub tools: PathBuf,
    downloads: PathBuf,
}

impl Paths {
    pub fn new() -> io::Result<Self> {
        let base =
            BaseDirs::new().ok_or_else(|| io::Error::other("Папка профиля Windows недоступна"))?;
        let roaming = env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| base.config_dir().to_owned());
        let local = env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| base.data_local_dir().to_owned())
            .join(APP_NAME);
        let data = roaming.join(APP_NAME);
        let downloads = UserDirs::new()
            .and_then(|dirs| dirs.video_dir().map(Path::to_owned))
            .unwrap_or_else(|| base.home_dir().join("Videos"))
            .join(APP_NAME);
        Ok(Self {
            settings: data.join("settings.json"),
            log: data.join("app.log"),
            legacy_webview: roaming.join("pywebview"),
            webview: local.join("webview"),
            tools: local.join("tools"),
            downloads,
        })
    }

    pub fn load_settings(&self) -> Settings {
        let mut settings = Settings::new(self.downloads.clone());
        match fs::read(&self.settings) {
            Ok(bytes) => {
                if let Err(error) = settings.load_json(&bytes) {
                    self.log(format!("settings read failed: {error}"));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => self.log(format!("settings read failed: {error}")),
        }
        settings
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        let directory = self.settings.parent().expect("settings directory");
        fs::create_dir_all(directory)?;
        let mut file = NamedTempFile::new_in(directory)?;
        serde_json::to_writer_pretty(&mut file, settings)?;
        writeln!(file)?;
        file.as_file().sync_all()?;
        file.persist(&self.settings).map_err(|error| error.error)?;
        Ok(())
    }

    pub fn log(&self, message: impl AsRef<str>) {
        let directory = self.log.parent().expect("log directory");
        let _ = fs::create_dir_all(directory);
        if fs::metadata(&self.log).is_ok_and(|info| info.len() > 200_000) {
            let _ = fs::rename(&self.log, self.log.with_extension("log.old"));
        }
        if let Ok(mut log) = OpenOptions::new().create(true).append(true).open(&self.log) {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let line = format!("[{now}] {}\n", message.as_ref());
            let _ = log.write_all(line.as_bytes());
        }
    }

    pub fn migrate_profile(&self) -> io::Result<()> {
        if self.webview.exists() || !self.legacy_webview.is_dir() {
            return Ok(());
        }
        let parent = self.webview.parent().expect("WebView2 data directory");
        fs::create_dir_all(parent)?;
        let staging = TempDir::new_in(parent)?;
        copy_profile(&self.legacy_webview, staging.path())?;
        fs::rename(staging.path(), &self.webview)?;
        self.log("legacy WebView2 profile migrated");
        Ok(())
    }
}

fn copy_profile(from: &Path, to: &Path) -> io::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir(&target)?;
            copy_profile(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain([0]).collect()
}

pub fn message_box(text: &str) {
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            wide(text).as_ptr(),
            wide(APP_NAME).as_ptr(),
            MB_OK | MB_ICONERROR | MB_TOPMOST,
        );
    }
}

pub struct Instance(HANDLE);

impl Instance {
    pub fn acquire() -> io::Result<Option<Self>> {
        let name = wide(MUTEX_NAME);
        let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let instance = Self(handle);
        Ok((!already_running).then_some(instance))
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

extern "system" fn match_window(window: HWND, data: LPARAM) -> i32 {
    let mut text = [0u16; 128];
    let length = unsafe { GetWindowTextW(window, text.as_mut_ptr(), text.len() as i32) };
    if length <= 0 || unsafe { IsWindowVisible(window) } == 0 {
        return 1;
    }
    let title = String::from_utf16_lossy(&text[..length as usize]);
    if !title.starts_with("MaklyxaTube ") {
        return 1;
    }
    let found = data as *mut c_void as *mut HWND;
    unsafe { *found = window };
    0
}

pub fn activate_existing_window() -> bool {
    for _ in 0..20 {
        let mut window: HWND = ptr::null_mut();
        unsafe {
            EnumWindows(Some(match_window), ptr::from_mut(&mut window) as LPARAM);
            if !window.is_null() {
                ShowWindow(window, SW_RESTORE);
                SetForegroundWindow(window);
                return true;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}
