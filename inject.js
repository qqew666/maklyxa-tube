(function () {
  if (window.__MT_INJECTED) return;
  window.__MT_INJECTED = true;

  // ---------- theme: dark + red accent (как на скрине) ----------
  var THEME_CSS = [
    "html { background:#0f0f0f !important; }",
    "html[dark], ytd-app[dark] { --yt-spec-brand-background-primary:#0f0f0f; }",
    "html { --yt-spec-call-to-action:#ff0033 !important; ",
    "  --yt-spec-brand-button-background:#ff0033 !important; ",
    "  --yt-spec-red-indicator:#ff0033 !important; ",
    "  --yt-spec-static-brand-red:#ff0033 !important; ",
    "  --yt-spec-static-brand-white:#ff3355 !important; }",
    "ytd-app { --yt-spec-call-to-action:#ff0033 !important; ",
    "  --yt-spec-brand-button-background:#ff0033 !important; ",
    "  --yt-spec-red-indicator:#ff0033 !important; ",
    "  --yt-spec-static-brand-red:#ff0033 !important; }",
    "#logo-icon svg path[fill='#FF0033'], #logo-icon svg path[fill='#ff0033'] { fill:#ff0033 !important; }",
    ".ytp-play-progress { background:#ff0033 !important; }",
    ".ytp-swatch-background-color { background:#ff0033 !important; }",
    ".ytp-load-progress { background:rgba(255,0,51,0.35) !important; }",
    "yt-searchbox input { caret-color:#ff0033 !important; }",
    ".yt-spec-button-shape-next--filled { background:#ff0033 !important; }",
    ".yt-spec-button-shape-next--call-to-action { color:#ff0033 !important; }",
    "::-webkit-scrollbar { width:10px; height:10px; }",
    "::-webkit-scrollbar-track { background:#0f0f0f; }",
    "::-webkit-scrollbar-thumb { background:#3a0d16; border-radius:6px; border:2px solid #0f0f0f; }",
    "::-webkit-scrollbar-thumb:hover { background:#ff0033; }",
    // прячем рекламные блоки страницы
    "#masthead-ad, .ytd-display-ad-renderer, ytd-promoted-video-renderer,",
    "ytd-banner-promo-renderer, .ytd-statement-banner-renderer,",
    "ytd-ad-slot-renderer, ytd-in-feed-ad-layout-renderer { display:none !important; }"
  ].join("\n");

  function ensureDark() {
    if (!document.documentElement.hasAttribute("dark")) {
      document.documentElement.setAttribute("dark", "true");
    }
  }
  function applyTheme() {
    var st = document.getElementById("__mt_theme");
    if (!st) {
      st = document.createElement("style");
      st.id = "__mt_theme";
      document.documentElement.appendChild(st);
    }
    if (st.textContent !== THEME_CSS) st.textContent = THEME_CSS;
    ensureDark();
  }
  applyTheme();
  new MutationObserver(applyTheme).observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["dark", "style"]
  });

  // ---------- настройки из приложения ----------
  window.__MT_SETTINGS = window.__MT_SETTINGS || {};

  // ---------- пропуск рекламы ----------
  setInterval(function () {
    if (window.__MT_SETTINGS.adblock === false) return;
    var skip = document.querySelector(".ytp-skip-ad-button, .ytp-skip-ad-button-modern");
    if (skip) { skip.click(); return; }
    var ad = document.querySelector(".ad-showing");
    var v = document.querySelector("video");
    if (ad && v && isFinite(v.duration) && v.duration > 0) {
      v.muted = true;
      try { v.currentTime = v.duration; } catch (e) {}
    }
    document.querySelectorAll(".ytp-ad-overlay-close-button").forEach(function (b) { b.click(); });
  }, 350);

  // ---------- SponsorBlock: пропуск спонсорских вставок ----------
  var sbSegments = null;
  var sbVideoId = null;
  var sbFetching = false;

  function videoIdFromUrl() {
    var m = location.href.match(/[?&]v=([\w-]{11})/);
    return m ? m[1] : null;
  }

  async function sbFetch(vid) {
    sbFetching = true;
    sbSegments = null;
    try {
      var cats = (window.__MT_SETTINGS.categories || ["sponsor", "selfpromo"])
        .map(function (c) { return '"' + c + '"'; }).join(",");
      var hash = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(vid));
      var prefix = Array.from(new Uint8Array(hash).slice(0, 4))
        .map(function (b) { return b.toString(16).padStart(2, "0"); }).join("");
      var r = await fetch(
        "https://sponsor.ajay.app/api/skipSegments/" + prefix +
        "?categories=[" + encodeURIComponent(cats) + "]"
      );
      if (!r.ok) { sbFetching = false; return; }
      var data = await r.json();
      for (var i = 0; i < data.length; i++) {
        if (data[i].videoID === vid) {
          sbSegments = data[i].segments.filter(function (s) {
            return s.segment && s.segment.length === 2;
          });
          break;
        }
      }
    } catch (e) {}
    sbFetching = false;
  }

  setInterval(function () {
    if (window.__MT_SETTINGS.sponsorblock === false) return;
    var vid = videoIdFromUrl();
    if (!vid) { sbSegments = null; sbVideoId = null; return; }
    if (vid !== sbVideoId) { sbVideoId = vid; sbFetch(vid); }
  }, 1000);

  document.addEventListener("timeupdate", function (e) {
    if (window.__MT_SETTINGS.sponsorblock === false) return;
    var v = e.target;
    if (!v || !v.duration || !sbSegments) return;
    for (var i = 0; i < sbSegments.length; i++) {
      var s = sbSegments[i].segment;
      if (v.currentTime >= s[0] && v.currentTime < s[1] - 0.05) {
        v.currentTime = s[1];
        mtToast("Пропущен фрагмент: " + (sbSegments[i].category || "sponsor"));
        break;
      }
    }
  }, true);

  // ---------- панель MaklyxaTube ----------
  function mtToast(text) {
    var t = document.createElement("div");
    t.textContent = text;
    t.style.cssText =
      "position:fixed;left:16px;bottom:76px;z-index:999999;background:#1a1a1a;" +
      "color:#fff;border-left:3px solid #ff0033;padding:10px 16px;border-radius:6px;" +
      "font:13px Roboto,Arial;box-shadow:0 4px 18px rgba(0,0,0,.6);max-width:340px;";
    document.body.appendChild(t);
    setTimeout(function () { t.remove(); }, 3500);
  }

  function mtPanel() {
    var old = document.getElementById("__mt_panel");
    if (old) { old.remove(); return; }
    var p = document.createElement("div");
    p.id = "__mt_panel";
    var row = function (key, label) {
      return '<label style="display:flex;gap:8px;align-items:center;padding:5px 0;cursor:pointer">' +
        '<input type="checkbox" data-k="' + key + '"' +
        (window.__MT_SETTINGS[key] !== false ? " checked" : "") + ">" + label + "</label>";
    };
    p.innerHTML =
      '<div style="position:fixed;top:64px;right:16px;z-index:999998;width:250px;' +
      'background:#141414;border:1px solid #ff003355;border-radius:10px;padding:14px 16px;' +
      'color:#eee;font:13px Roboto,Arial;box-shadow:0 8px 30px rgba(0,0,0,.7)">' +
      '<div style="font-weight:700;color:#ff0033;margin-bottom:8px">MaklyxaTube</div>' +
      row("adblock", "Без рекламы") +
      row("sponsorblock", "Пропуск спонсорских вставок") +
      '<div style="margin-top:10px;border-top:1px solid #333;padding-top:8px;color:#aaa;font-size:12px">' +
      "Скачанные видео — в папке Видео\\MaklyxaTube</div></div>";
    document.body.appendChild(p);
    p.querySelectorAll("input").forEach(function (inp) {
      inp.addEventListener("change", function () {
        window.pywebview.api.set_setting(inp.dataset.k, inp.checked);
        window.__MT_SETTINGS[inp.dataset.k] = inp.checked;
      });
    });
    setTimeout(function () {
      document.addEventListener("click", function h(ev) {
        if (p && !p.contains(ev.target) && !("__mt_btn" in (ev.target.dataset || {}))) {
          p.remove();
          document.removeEventListener("click", h);
        }
      });
    }, 0);
  }

  // панель кнопок, скрыта до Shift+F1
  function mtBar() {
    if (document.getElementById("__mt_bar")) return;
    var bar = document.createElement("div");
    bar.id = "__mt_bar";
    bar.style.cssText =
      "position:fixed;top:12px;right:16px;z-index:999997;display:none;" +
      "gap:8px;align-items:center;";

    var btn = function (id, text) {
      var b = document.createElement("button");
      b.id = id;
      b.textContent = text;
      b.style.cssText =
        "padding:7px 12px;border-radius:8px;border:1px solid #ff0033;background:#141414;" +
        "color:#fff;font:600 12px Roboto,Arial;cursor:pointer;box-shadow:0 4px 14px rgba(0,0,0,.5);";
      return b;
    };

    var mt = btn("__mt_btn", "MT");
    mt.title = "Настройки MaklyxaTube";
    mt.style.color = "#ff0033";
    mt.style.fontWeight = "700";
    mt.addEventListener("click", function (ev) { ev.stopPropagation(); mtPanel(); });
    bar.appendChild(mt);

    var d = btn("__mt_dl", "⬇ Скачать");
    d.title = "Скачать это видео";
    d.addEventListener("click", function () {
      var url = location.href;
      if (url.indexOf("watch?v=") === -1 && url.indexOf("youtu.be/") === -1) {
        mtToast("Открой видео, чтобы скачать его");
        return;
      }
      window.pywebview.api.download(url, "mp4").then(function (ok) {
        if (ok) {
          window.__MT_DL_ACTIVE = true;
          mtToast("Скачивание началось…");
        } else {
          mtToast("Скачивание уже идёт или ссылка не подходит");
        }
      });
    });
    bar.appendChild(d);

    document.body.appendChild(bar);
  }

  function toggleBar() {
    mtBar();
    var bar = document.getElementById("__mt_bar");
    if (!bar) return;
    var show = bar.style.display === "none";
    bar.style.display = show ? "flex" : "none";
    if (!show) {
      var p = document.getElementById("__mt_panel");
      if (p) p.remove();
    }
  }

  document.addEventListener("keydown", function (e) {
    if (e.shiftKey && (e.key === "F1" || e.keyCode === 112)) {
      e.preventDefault();
      e.stopPropagation();
      toggleBar();
    }
  }, true);

  // ---------- скачивание: статус получаем опросом из Python ----------
  window.__MT_DL_ACTIVE = false;
  setInterval(function () {
    if (!window.__MT_DL_ACTIVE) return;
    var api = window.pywebview && window.pywebview.api;
    if (!api) return;
    api.get_download_state().then(function (st) {
      var d = document.getElementById("__mt_dl");
      if (!d) return;
      if (st.error) {
        window.__MT_DL_ACTIVE = false;
        d.textContent = "⬇ Скачать";
        mtToast("Ошибка скачивания: " + st.error);
      } else if (!st.active) {
        window.__MT_DL_ACTIVE = false;
        d.textContent = "⬇ Скачать";
        mtToast("Скачано: " + (st.title || "видео"));
      } else {
        d.textContent = st.pct >= 0 ? "⬇ " + st.pct + "%" : "⬇ …";
      }
    }).catch(function () {});
  }, 700);

  setInterval(function () {
    if (!document.body) return;
    mtBar();
    applyTheme();
  }, 2000);
  mtBar();

  // ---------- автопереподключение, если YouTube не открылся ----------
  window.__MT_NET_TRIES = 0;
  function netTick() {
    var host = location.hostname;
    if (host === "www.youtube.com" || host === "youtube.com" || host === "m.youtube.com") {
      window.__MT_NET_TRIES = 0;
      return;
    }
    window.__MT_NET_TRIES++;
    var tries = window.__MT_NET_TRIES;
    if (tries === 3) {
      mtToast("Нет связи с YouTube. Проверь интернет или VPN — переподключаюсь…");
    }
    if (tries === 1 || tries % 4 === 0) {
      setTimeout(function () { try { location.reload(); } catch (e) {} }, 1500);
    }
  }
  setInterval(netTick, 4000);

  // применяем настройки из нативного приложения
  try {
    if (window.pywebview && window.pywebview.api) {
      window.pywebview.api.get_settings().then(function (s) {
        window.__MT_SETTINGS = s || {};
      });
    }
  } catch (e) {}
})();
