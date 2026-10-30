import ctypes
import json
import os
import shutil
import socket
import subprocess
import sys
import threading
import time
import traceback

import webview

APP_NAME = "MaklyxaTube"
VERSION = "0.1.1"

APP_DATA_DIR = os.path.join(os.environ.get("APPDATA") or os.path.expanduser("~"), APP_NAME)
CONFIG_PATH = os.path.join(APP_DATA_DIR, "settings.json")
LOG_PATH = os.path.join(APP_DATA_DIR, "app.log")
WEBVIEW_DIR = os.path.join(
    os.environ.get("LOCALAPPDATA") or os.path.expanduser("~"), APP_NAME, "webview"
)
LEGACY_WEBVIEW_DIR = os.path.join(
    os.environ.get("APPDATA") or os.path.expanduser("~"), "pywebview"
)
DEFAULT_DOWNLOAD_DIR = os.path.join(os.path.expanduser("~"), "Videos", APP_NAME)

MUTEX_NAME = "Local\\MaklyxaTube.SingleInstance"

DEFAULT_SETTINGS = {
    "adblock": True,
    "sponsorblock": True,
    "categories": ["sponsor", "selfpromo"],
    "downloadDir": DEFAULT_DOWNLOAD_DIR,
}


def resource_path(name):
    base = getattr(sys, "_MEIPASS", os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(base, name)


def log(msg):
    try:
        os.makedirs(APP_DATA_DIR, exist_ok=True)
        if os.path.exists(LOG_PATH) and os.path.getsize(LOG_PATH) > 200_000:
            os.replace(LOG_PATH, LOG_PATH + ".old")
        stamp = time.strftime("%Y-%m-%d %H:%M:%S")
        with open(LOG_PATH, "a", encoding="utf-8") as f:
            f.write("[%s] %s\n" % (stamp, msg))
    except Exception:
        pass


def message_box(text, flags=0x00000040 | 0x00040000):
    try:
        ctypes.windll.user32.MessageBoxW(None, text, APP_NAME, flags)
    except Exception:
        pass


# ---------- один экземпляр ----------
_kernel32 = None
_inst_mutex = None


def _k32():
    global _kernel32
    if _kernel32 is None:
        _kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    return _kernel32


def acquire_single_instance():
    global _inst_mutex
    try:
        _inst_mutex = _k32().CreateMutexW(None, False, MUTEX_NAME)
        if ctypes.get_last_error() == 183:  # ERROR_ALREADY_EXISTS
            return False
        return True
    except Exception:
        log("mutex failed:\n" + traceback.format_exc())
        return True


def release_single_instance():
    global _inst_mutex
    try:
        if _inst_mutex:
            _k32().CloseHandle(_inst_mutex)
            _inst_mutex = None
    except Exception:
        pass


def _find_window(prefix):
    u = ctypes.WinDLL("user32")
    found = []
    WNDENUMPROC = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)

    def _cb(hwnd, param):
        if not u.IsWindowVisible(hwnd):
            return True
        n = u.GetWindowTextLengthW(hwnd)
        if n <= 0:
            return True
        buf = ctypes.create_unicode_buffer(n + 1)
        u.GetWindowTextW(hwnd, buf, n + 1)
        if buf.value.startswith(prefix):
            found.append(hwnd)
            return False
        return True

    u.EnumWindows(WNDENUMPROC(_cb), 0)
    return found[0] if found else 0


def activate_existing_window():
    try:
        u = ctypes.WinDLL("user32")
        hwnd = _find_window(APP_NAME)
        if not hwnd:
            return False
        u.ShowWindow(hwnd, 9)  # SW_RESTORE
        u.SetForegroundWindow(hwnd)
        return True
    except Exception:
        return False


# ---------- профиль WebView2 ----------
def migrate_profile():
    try:
        if os.path.isdir(WEBVIEW_DIR) or not os.path.isdir(LEGACY_WEBVIEW_DIR):
            return
        os.makedirs(os.path.dirname(WEBVIEW_DIR), exist_ok=True)
        shutil.copytree(LEGACY_WEBVIEW_DIR, WEBVIEW_DIR, dirs_exist_ok=True)
        log("profile migrated from %s" % LEGACY_WEBVIEW_DIR)
    except Exception:
        log("profile migrate failed:\n" + traceback.format_exc())


# ---------- настройки ----------
def load_settings():
    cfg = dict(DEFAULT_SETTINGS)
    try:
        with open(CONFIG_PATH, "r", encoding="utf-8") as f:
            data = json.load(f)
        if isinstance(data, dict):
            cfg.update({k: v for k, v in data.items() if k in DEFAULT_SETTINGS})
        cfg["categories"] = [c for c in cfg.get("categories", []) if isinstance(c, str)]
    except Exception:
        pass
    return cfg


def save_settings(cfg):
    try:
        os.makedirs(APP_DATA_DIR, exist_ok=True)
        with open(CONFIG_PATH, "w", encoding="utf-8") as f:
            json.dump(cfg, f, ensure_ascii=False, indent=2)
    except Exception:
        pass


class Api:
    def __init__(self):
        self.settings = load_settings()
        self._dl = {"active": False, "pct": 0, "title": "", "error": ""}
        self._dl_lock = threading.Lock()

    def get_settings(self):
        return dict(self.settings)

    def set_setting(self, key, value):
        if key not in DEFAULT_SETTINGS:
            return False
        if key == "categories" and not isinstance(value, list):
            return False
        self.settings[key] = value
        save_settings(self.settings)
        return True

    def get_download_state(self):
        with self._dl_lock:
            return dict(self._dl)

    def download(self, url, mode="mp4"):
        if not isinstance(url, str):
            return False
        if "youtube.com/watch" not in url and "youtu.be/" not in url:
            return False
        with self._dl_lock:
            if self._dl["active"]:
                return False
            self._dl = {"active": True, "pct": 0, "title": "", "error": ""}
        threading.Thread(target=self._download, args=(url, mode), daemon=True).start()
        return True

    def _set_dl(self, **kw):
        with self._dl_lock:
            self._dl.update(kw)

    def _download(self, url, mode):
        try:
            import yt_dlp
            from imageio_ffmpeg import get_ffmpeg_exe

            out_dir = self.settings.get("downloadDir") or DEFAULT_DOWNLOAD_DIR
            os.makedirs(out_dir, exist_ok=True)
            ffmpeg = None
            try:
                cand = get_ffmpeg_exe()
                if cand and os.path.exists(cand):
                    ffmpeg = cand
            except Exception:
                ffmpeg = None
            if not ffmpeg:
                ffmpeg = shutil.which("ffmpeg")
            have_ffmpeg = bool(ffmpeg)
            if mode == "mp4":
                fmt = "bv*+ba/b" if have_ffmpeg else "b[ext=mp4]/b"
            else:
                fmt = "ba/b"

            def hook(d):
                if d.get("status") != "downloading":
                    return
                total = d.get("total_bytes") or d.get("total_bytes_estimate") or 0
                done = d.get("downloaded_bytes") or 0
                pct = int(done * 100 / total) if total else -1
                self._set_dl(pct=pct)

            opts = {
                "format": fmt,
                "outtmpl": os.path.join(out_dir, "%(title)s.%(ext)s"),
                "noplaylist": True,
                "quiet": True,
                "no_warnings": True,
                "noprogress": True,
                "progress_hooks": [hook],
            }
            node = shutil.which("node")
            if node:
                opts["js_runtimes"] = {"node": {"path": node}}
            if have_ffmpeg:
                opts["ffmpeg_location"] = ffmpeg
            if mode == "mp4" and have_ffmpeg:
                opts["merge_output_format"] = "mp4"
            if mode == "mp3" and have_ffmpeg:
                opts["postprocessors"] = [
                    {"key": "FFmpegExtractAudio", "preferredcodec": "mp3"}
                ]
            with yt_dlp.YoutubeDL(opts) as ydl:
                info = ydl.extract_info(url, download=True)
                title = info.get("title", "video")
            self._set_dl(active=False, pct=100, title=title)
            log("download ok: %s" % title)
        except Exception as exc:
            msg = str(exc).replace("\n", " ")[:300]
            self._set_dl(active=False, error=msg)
            log("download failed: %s" % msg)


INJECT_JS = ""


def on_loaded(window):
    try:
        window.evaluate_js(INJECT_JS)
        log("page loaded, injected")
    except Exception:
        log("inject failed:\n" + traceback.format_exc())


def watchdog(state):
    for _ in range(90):
        time.sleep(1)
        if state.get("loaded"):
            return
    log("WARN: page did not load in 90s")
    message_box(
        "YouTube не загрузился.\nЗакрой окно и запусти MaklyxaTube ещё раз.\n\nЛог: "
        + LOG_PATH
    )


def network_hint():
    for _ in range(3):
        try:
            socket.create_connection(("www.youtube.com", 443), timeout=4).close()
            return
        except OSError:
            time.sleep(1.0)
    log("no network to www.youtube.com")
    message_box(
        "Нет соединения с YouTube (DNS или сеть).\n"
        "Проверь интернет или включи VPN — приложение будет переподключаться само."
    )


def restart_self():
    try:
        env = {k: v for k, v in os.environ.items() if not k.startswith("_PYI")}
        env["MAKLYXA_RESTARTED"] = "1"
        subprocess.Popen([sys.executable], env=env, close_fds=True)
        return True
    except Exception:
        log("restart failed:\n" + traceback.format_exc())
        return False


def main():
    global INJECT_JS

    log("start %s" % VERSION)

    if not acquire_single_instance():
        if not activate_existing_window():
            message_box("MaklyxaTube уже запущен.")
        log("second instance -> activate existing")
        return

    try:
        with open(resource_path("inject.js"), "r", encoding="utf-8") as f:
            INJECT_JS = f.read()
    except Exception:
        log("inject.js read failed:\n" + traceback.format_exc())

    migrate_profile()

    api = Api()
    state = {"loaded": False}
    threading.Thread(target=watchdog, args=(state,), daemon=True).start()
    threading.Thread(target=network_hint, daemon=True).start()

    window = webview.create_window(
        "%s %s" % (APP_NAME, VERSION),
        "https://www.youtube.com/",
        js_api=api,
        width=1380,
        height=860,
        min_size=(900, 560),
        background_color="#0f0f0f",
    )

    def loaded():
        state["loaded"] = True
        on_loaded(window)

    window.events.loaded += loaded

    try:
        webview.start(private_mode=False, storage_path=WEBVIEW_DIR, gui="edgechromium")
        log("exit")
    except Exception:
        log("start failed:\n" + traceback.format_exc())
        release_single_instance()
        if not os.environ.get("MAKLYXA_RESTARTED") and restart_self():
            log("restarting after failure")
            return
        message_box(
            "Не удалось запустить MaklyxaTube.\nЗапусти приложение ещё раз.\n\nЛог: "
            + LOG_PATH
        )


if __name__ == "__main__":
    main()
