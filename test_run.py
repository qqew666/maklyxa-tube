import os
import sys

os.environ["APPDATA"] = os.path.join(os.path.dirname(__file__), "_test_appdata")
os.environ["LOCALAPPDATA"] = os.path.join(os.path.dirname(__file__), "_test_localappdata")
os.makedirs(os.environ["APPDATA"], exist_ok=True)
os.makedirs(os.environ["LOCALAPPDATA"], exist_ok=True)

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import app
import webview

with open(app.resource_path("inject.js"), "r", encoding="utf-8") as f:
    app.INJECT_JS = f.read()

TARGET = os.environ.get("MT_TARGET", "splash")


class ProbeApi(app.Api):
    def report(self, payload):
        try:
            print("RESULT " + str(payload), flush=True)
        finally:
            os._exit(0)

    def fail(self, msg):
        print("FAILED " + str(msg), flush=True)
        os._exit(1)


REPORT_AT = int(os.environ.get("MT_REPORT_AT", "5000"))

REPORT_JS = r"""
function mtReport() {
  try {
    var bar = document.getElementById('__mt_bar');
    var before = bar ? bar.style.display : 'missing';
    document.dispatchEvent(new KeyboardEvent('keydown', {key:'F1', shiftKey:true, bubbles:true}));
    var after = bar ? bar.style.display : 'missing';
    var off = document.getElementById('__mt_offline');
    var card = document.getElementById('card');
    var payload = {
      href: location.href.slice(0, 90),
      proto: location.protocol,
      injected: !!window.__MT_INJECTED,
      splash: !!document.querySelector('.s'),
      cardShown: card ? card.style.display : 'none',
      cap: (document.getElementById('cap') || {}).textContent || '',
      statusText: (document.getElementById('st') || {}).textContent || '',
      text: (document.body ? document.body.innerText : '').replace(/\s+/g, ' ').slice(0, 120),
      offline: !!off,
      offlineStatus: (document.getElementById('__mt_offline_status') || {}).textContent || '',
      bar: !!bar,
      before: before,
      after: after,
      dark: document.documentElement.getAttribute('dark'),
      api: !!(window.pywebview && window.pywebview.api)
    };
    window.pywebview.api.report(JSON.stringify(payload));
  } catch (e) {
    try { window.pywebview.api.fail(String(e)); } catch (e2) {}
  }
}
setTimeout(mtReport, __MT_REPORT_AT__);
""".replace("__MT_REPORT_AT__", str(REPORT_AT))


def on_loaded(window):
    print("LOADED fired", flush=True)
    try:
        window.evaluate_js(app.INJECT_JS)
        window.evaluate_js(REPORT_JS)
        print("INJECTED js", flush=True)
    except Exception as exc:
        print("LOADERR " + repr(exc), flush=True)
        os._exit(1)


api = ProbeApi()
if TARGET == "splash":
    window = webview.create_window(
        "MT-test", html=app.SPLASH_HTML, js_api=api, width=1200, height=800
    )
elif TARGET == "splash_local":
    repl = os.environ.get("MT_REPLACE", "http://127.0.0.1:9/")
    window = webview.create_window(
        "MT-test",
        html=app.SPLASH_HTML.replace("https://www.youtube.com/", repl),
        js_api=api,
        width=1200,
        height=800,
    )
else:
    window = webview.create_window(
        "MT-test", TARGET, js_api=api, width=1200, height=800
    )
window.events.loaded += lambda: on_loaded(window)
webview.start(private_mode=False, storage_path=app.WEBVIEW_DIR)
