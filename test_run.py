import json
import os
import sys

os.environ["APPDATA"] = os.path.join(os.path.dirname(__file__), "_test_appdata")
os.makedirs(os.environ["APPDATA"], exist_ok=True)

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import app
import webview

with open(app.resource_path("inject.js"), "r", encoding="utf-8") as f:
    app.INJECT_JS = f.read()


class ProbeApi(app.Api):
    def report(self, payload):
        try:
            print("RESULT " + str(payload), flush=True)
        finally:
            os._exit(0)

    def fail(self, msg):
        print("FAILED " + str(msg), flush=True)
        os._exit(1)


REPORT_JS = r"""
function mtReport() {
  try {
    var v = document.querySelector('video');
    var payload = {
      href: location.href.slice(0, 80),
      bridge: typeof (window.pywebview && window.pywebview.api && window.pywebview.api.report),
      title: document.title,
      injected: !!window.__MT_INJECTED,
      theme: !!document.getElementById('__mt_theme'),
      btn: !!document.getElementById('__mt_btn'),
      dl: !!document.getElementById('__mt_dl'),
      dark: document.documentElement.getAttribute('dark'),
      api: !!(window.pywebview && window.pywebview.api),
      adCss: (function () {
        var st = document.getElementById('__mt_theme');
        return st ? st.textContent.indexOf('masthead-ad') !== -1 : false;
      })(),
      video: v ? { paused: v.paused, t: v.currentTime } : null
    };
    window.pywebview.api.report(JSON.stringify(payload));
  } catch (e) {
    try { window.pywebview.api.fail(String(e)); } catch (e2) {}
  }
}
setTimeout(mtReport, 6000);
setTimeout(mtReport, 15000);
"""


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
window = webview.create_window(
    "MT-test", "https://www.youtube.com/", js_api=api, width=1200, height=800
)
window.events.loaded += lambda: on_loaded(window)
webview.start(private_mode=False)
