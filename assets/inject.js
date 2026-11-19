(function () {
  if (window.top !== window || window.__MT_INJECTED) return;
  if (location.protocol !== "https:" || location.port ||
      !["youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be"].includes(location.hostname)) return;

  function init() {
    if (window.__MT_INJECTED || !window.chrome?.webview) return;
    window.__MT_INJECTED = true;
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
      "::-webkit-scrollbar-thumb:hover { background:#ff0033; }"
    ].join("\n");

    var AD_CSS = [
      "#masthead-ad, .ytd-display-ad-renderer, ytd-promoted-video-renderer,",
      "ytd-banner-promo-renderer, .ytd-statement-banner-renderer,",
      "ytd-ad-slot-renderer, ytd-in-feed-ad-layout-renderer { display:none !important; }"
    ].join("\n");

    function setStyle(id, css) {
      var style = document.getElementById(id);
      if (!style) {
        style = document.createElement("style");
        style.id = id;
        document.documentElement.appendChild(style);
      }
      if (style.textContent !== css) style.textContent = css;
      return style;
    }

    function applyTheme() {
      setStyle("__mt_theme", THEME_CSS);
      var adStyle = setStyle("__mt_ad_style", AD_CSS);
      adStyle.disabled = window.__MT_SETTINGS.adblock !== true;
      if (!document.documentElement.hasAttribute("dark")) {
        document.documentElement.setAttribute("dark", "true");
      }
    }

    function send(message) {
      window.chrome.webview.postMessage(JSON.stringify(message));
    }

    window.__MT_SETTINGS = {};
    applyTheme();
    new MutationObserver(applyTheme).observe(document.documentElement, {
      attributes: true, attributeFilter: ["dark", "style"]
    });

    var mutedForAd = null;
    setInterval(function () {
      var enabled = window.__MT_SETTINGS.adblock === true;
      var ad = enabled && document.querySelector(".ad-showing");
      var video = ad && ad.querySelector("video");
      if (mutedForAd && mutedForAd !== video) {
        mutedForAd.muted = false;
        mutedForAd = null;
      }
      if (!enabled) return;
      var skip = document.querySelector(".ytp-skip-ad-button, .ytp-skip-ad-button-modern");
      if (skip && !skip.disabled) { skip.click(); return; }
      if (video && Number.isFinite(video.duration) && video.duration > 0) {
        if (!video.muted) {
          mutedForAd = video;
          video.muted = true;
        }
        try { video.currentTime = video.duration; } catch (_) {}
      }
      document.querySelectorAll(".ytp-ad-overlay-close-button").forEach(function (button) {
        button.click();
      });
    }, 350);

    function videoIdFromUrl() {
      var id = location.hostname === "youtu.be" ? location.pathname.slice(1) :
        (location.pathname === "/watch" ? new URLSearchParams(location.search).get("v") : null);
      return id && /^[A-Za-z0-9_-]{11}$/.test(id) ? id : null;
    }

    var sponsorRequest = null;
    function updateSponsors() {
      var vid = window.__MT_SETTINGS.sponsorblock === true && videoIdFromUrl();
      var categories = window.__MT_SETTINGS.categories || [];
      var key = vid && categories.length ? JSON.stringify([vid, categories]) : null;
      if (sponsorRequest && sponsorRequest.key === key && Date.now() < sponsorRequest.retryAt) return;
      if (sponsorRequest) sponsorRequest.controller.abort();
      sponsorRequest = null;
      if (!key) return;
      sponsorRequest = { key: key, vid: vid, controller: new AbortController(), segments: [], retryAt: Infinity };
      fetchSponsors(sponsorRequest, categories);
    }

    async function fetchSponsors(request, categories) {
      var timeout = setTimeout(function () { request.controller.abort(); }, 10000);
      try {
        var hash = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(request.vid));
        var prefix = Array.from(new Uint8Array(hash).slice(0, 2))
          .map(function (byte) { return byte.toString(16).padStart(2, "0"); }).join("");
        var query = new URLSearchParams({ categories: JSON.stringify(categories) });
        var response = await fetch("https://sponsor.ajay.app/api/skipSegments/" + prefix + "?" + query,
          { signal: request.controller.signal });
        if (response.status === 404) return;
        if (!response.ok) throw new Error("SponsorBlock " + response.status);
        var videos = await response.json();
        var match = videos.find(function (entry) { return entry.videoID === request.vid; });
        if (match) {
          request.segments = match.segments.filter(function (entry) {
            return Array.isArray(entry.segment) && entry.segment.length === 2 &&
              entry.segment.every(Number.isFinite);
          });
        }
      } catch (_) {
        request.retryAt = Date.now() + 30000;
      } finally {
        clearTimeout(timeout);
      }
    }

    setInterval(updateSponsors, 1000);
    document.addEventListener("timeupdate", function (event) {
      var video = event.target;
      if (!sponsorRequest || sponsorRequest.vid !== videoIdFromUrl() ||
          !(video instanceof HTMLVideoElement) || !video.duration ||
          video.closest(".ad-showing")) return;
      for (var entry of sponsorRequest.segments) {
        var segment = entry.segment;
        if (video.currentTime >= segment[0] && video.currentTime < segment[1] - 0.05) {
          video.currentTime = segment[1];
          mtToast("Пропущен фрагмент: " + (entry.category || "sponsor"));
          break;
        }
      }
    }, true);

    var routeRegion = null;
    var canSwitchRegion = false;
    var stalledPlayback = null;
    document.addEventListener("waiting", function (event) {
      var video = event.target;
      if (!canSwitchRegion || !(video instanceof HTMLVideoElement) ||
          !videoIdFromUrl() || video.paused || video.seeking || video.closest(".ad-showing")) return;
      clearTimeout(stalledPlayback);
      var start = video.currentTime;
      var href = location.href;
      stalledPlayback = setTimeout(function () {
        if (location.href === href && !video.paused && !video.seeking && video.readyState < 3 &&
            !video.closest(".ad-showing") && Math.abs(video.currentTime - start) < 0.25) {
          send({ type: "playback_stalled", position: Math.floor(start) });
        }
      }, 20000);
    }, true);
    for (var eventName of ["playing", "pause", "seeking"]) {
      document.addEventListener(eventName, function (event) {
        if (event.target instanceof HTMLVideoElement) clearTimeout(stalledPlayback);
      }, true);
    }

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
      if (old) { closePanel(); return; }
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
        '<div id="__mt_region"></div>' +
        "Скачанные видео — в папке Видео\\MaklyxaTube</div></div>";
      document.body.appendChild(p);
      p.querySelector("#__mt_region").textContent =
        routeRegion ? "Регион подключения: " + routeRegion : "Системное подключение";
      p.querySelectorAll("input").forEach(function (inp) {
        inp.addEventListener("change", function () {
          send({ type: "set_setting", key: inp.dataset.k, value: inp.checked });
        });
      });
      document.addEventListener("click", dismissPanel);
    }

    function dismissPanel(event) {
      var panel = document.getElementById("__mt_panel");
      if (panel && !panel.contains(event.target)) closePanel();
    }

    function closePanel() {
      var panel = document.getElementById("__mt_panel");
      if (panel) panel.remove();
      document.removeEventListener("click", dismissPanel);
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
        if (!videoIdFromUrl()) {
          mtToast("Открой видео, чтобы скачать его");
          return;
        }
        send({ type: "download", mode: "mp4" });
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
      if (!show) closePanel();
    }

    document.addEventListener("keydown", function (e) {
      if (e.shiftKey && (e.key === "F1" || e.keyCode === 112)) {
        e.preventDefault();
        e.stopPropagation();
        if (!e.repeat) toggleBar();
      }
    }, true);


    var receivedSettings = false;
    var downloadActive = false;
    function updateDownload(state, starting) {
      var button = document.getElementById("__mt_dl");
      if (state.active) {
        if (starting) mtToast("Скачивание началось…");
        downloadActive = true;
        if (button) button.textContent = state.pct >= 0 ? "⬇ " + state.pct + "%" : "⬇ …";
      } else {
        if (downloadActive || starting) {
          mtToast(state.error ? "Ошибка скачивания: " + state.error : "Скачано: " + (state.title || "видео"));
        }
        downloadActive = false;
        if (button) button.textContent = "⬇ Скачать";
      }
    }

    window.chrome.webview.addEventListener("message", function (event) {
      var message = event.data;
      if (!message || typeof message !== "object") return;
      if (message.type === "ready") {
        routeRegion = message.region;
        canSwitchRegion = message.canSwitchRegion === true;
      }
      if (message.type === "ready" || message.type === "settings") {
        receivedSettings = true;
        window.__MT_SETTINGS = message.settings;
        applyTheme();
        updateSponsors();
        document.querySelectorAll("#__mt_panel input").forEach(function (input) {
          input.checked = message.settings[input.dataset.k] !== false;
        });
        if (message.saved === false) mtToast("Не удалось сохранить настройки");
      }
      if (message.type === "download_start" && !message.started) {
        mtToast("Скачивание уже идёт или ссылка не подходит");
        if (!message.download.active) return;
      }
      if (message.download) updateDownload(message.download, message.started === true);
    });
    setInterval(function () {
      if (!receivedSettings) send({ type: "ready" });
      else if (downloadActive) send({ type: "download_state" });
    }, 700);

    mtBar();
    setInterval(function () {
      mtBar();
      applyTheme();
    }, 2000);
    send({ type: "ready" });
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init, { once: true });
  else init();
})();
