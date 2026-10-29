import json
import os
import sys
import threading
import traceback

import webview

APP_NAME = "MaklyxaTube"
VERSION = "0.1.0"
CONFIG_DIR = os.path.join(os.environ.get("APPDATA") or os.path.expanduser("~"), APP_NAME)
CONFIG_PATH = os.path.join(CONFIG_DIR, "settings.json")
DEFAULT_DOWNLOAD_DIR = os.path.join(os.path.expanduser("~"), "Videos", APP_NAME)

DEFAULT_SETTINGS = {
    "adblock": True,
    "sponsorblock": True,
    "categories": ["sponsor", "selfpromo"],
    "downloadDir": DEFAULT_DOWNLOAD_DIR,
}


def resource_path(name):
    base = getattr(sys, "_MEIPASS", os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(base, name)


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
        os.makedirs(CONFIG_DIR, exist_ok=True)
        with open(CONFIG_PATH, "w", encoding="utf-8") as f:
            json.dump(cfg, f, ensure_ascii=False, indent=2)
    except Exception:
        pass


class Api:
    def __init__(self):
        self.settings = load_settings()
        self._dl = {"active": False, "pct": 0, "title": "", "error": ""}
        self._dl_lock = threading.Lock()

    # ---- settings (вызывается из JS-поллинга) ----
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

    # ---- downloads ----
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
            import shutil

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
        except Exception as exc:
            self._set_dl(active=False, error=str(exc).replace("\n", " ")[:300])


INJECT_JS = ""


def on_loaded(window):
    try:
        window.evaluate_js(INJECT_JS)
    except Exception:
        traceback.print_exc()


def main():
    global INJECT_JS
    with open(resource_path("inject.js"), "r", encoding="utf-8") as f:
        INJECT_JS = f.read()

    api = Api()
    window = webview.create_window(
        f"{APP_NAME} {VERSION}",
        "https://www.youtube.com/",
        js_api=api,
        width=1380,
        height=860,
        min_size=(900, 560),
    )
    window.events.loaded += lambda: on_loaded(window)
    webview.start(private_mode=False)


if __name__ == "__main__":
    main()
