use std::{
    env, fs,
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::json;
use url::Url;
use webview2_com::{
    Microsoft::Web::WebView2::Win32::{COREWEBVIEW2_WEB_ERROR_STATUS, ICoreWebView2},
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    WebMessageReceivedEventHandler, take_pwstr,
};
use windows::core::{BOOL, HSTRING, PWSTR};
use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::PostMessageW};
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

use crate::{
    download::Downloads,
    model::{PageMessage, Setting, Settings, YOUTUBE, is_youtube, video_url},
    platform::{APP_NAME, Instance, Paths, activate_existing_window, message_box},
    relay::Relay,
    tor::Readiness,
    window::{APP_EVENT, NativeMessage, Window},
};

const SPLASH: &str = include_str!("../assets/splash.html");
const INJECT: &str = include_str!("../assets/inject.js");
const LOAD_TIMEOUT: Duration = Duration::from_secs(18);
const PREPARATION_POLL: Duration = Duration::from_millis(350);
const PLAYBACK_SWITCH_INTERVAL: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(25);
const RESTARTED: &str = "MAKLYXA_RESTARTED";

type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

enum AppEvent {
    Starting {
        id: u64,
        url: String,
    },
    Completed {
        id: u64,
        result: Result<(), i32>,
    },
    Message {
        source: String,
        message: PageMessage,
    },
}

enum Connection {
    Loading { id: Option<u64>, deadline: Instant },
    Preparing { retry_at: Instant },
    Offline { retry_at: Option<Instant> },
    Browsing,
}

struct App {
    view: WebView,
    paths: Paths,
    settings: Settings,
    downloads: Downloads,
    connection: Connection,
    target: Url,
    attempts: u32,
    relay: Option<Relay>,
    invalid_routes: bool,
    failed_routes: usize,
    last_playback_switch: Option<Instant>,
}

pub fn run() {
    let paths = match Paths::new() {
        Ok(paths) => paths,
        Err(error) => {
            message_box(&error.to_string());
            return;
        }
    };
    let instance = match Instance::acquire() {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            if !activate_existing_window() {
                message_box("MaklyxaTube уже запущен.");
            }
            return;
        }
        Err(error) => {
            paths.log(format!("mutex failed: {error}"));
            message_box(&format!("Не удалось запустить MaklyxaTube.\n{error}"));
            return;
        }
    };

    paths.log(format!("start {}", env!("CARGO_PKG_VERSION")));
    if let Err(error) = paths.migrate_profile() {
        paths.log(format!("profile migration failed: {error}"));
    }

    let result = run_window(&paths);
    drop(instance);
    match result {
        Ok(()) => paths.log("exit"),
        Err(error) => {
            paths.log(format!("start failed: {error}"));
            if env::var_os(RESTARTED).is_none()
                && env::current_exe()
                    .and_then(|exe| Command::new(exe).env(RESTARTED, "1").spawn())
                    .is_ok()
            {
                paths.log("restarting after failure");
                return;
            }
            message_box(&format!(
                "Не удалось открыть MaklyxaTube. Проверь, установлен ли Microsoft Edge WebView2 Runtime.\n\n{error}\n\nЛог: {}",
                paths.log.display()
            ));
        }
    }
}

fn run_window(paths: &Paths) -> AppResult<()> {
    fs::create_dir_all(&paths.webview)?;
    let window = Window::new(&format!("{APP_NAME} {}", env!("CARGO_PKG_VERSION")))?;
    paths.log("Windows window created");
    let (relay, invalid_routes) = match paths
        .load_routes()
        .and_then(|routes| Relay::start(&routes, paths.tools.join("tor")))
    {
        Ok(relay) => (Some(relay), false),
        Err(error) => {
            paths.log(format!("route configuration failed: {error}"));
            (None, true)
        }
    };
    let mut browser_args = String::from(
        "--autoplay-policy=no-user-gesture-required --disable-background-timer-throttling --disable-renderer-backgrounding",
    );
    if let Some(relay) = &relay {
        browser_args.push_str(&relay.browser_args());
    }
    let mut context = WebContext::new(Some(paths.webview.clone()));
    let view = WebViewBuilder::new_with_web_context(&mut context)
        .with_background_color((15, 15, 15, 255))
        .with_additional_browser_args(browser_args)
        .with_initialization_script(INJECT)
        .build(&window)?;
    paths.log("WebView2 created");
    let (sender, events) = mpsc::channel();
    let proxy = EventProxy {
        window: window.handle(),
        sender,
    };
    attach_events(&view.webview(), &proxy)?;
    view.load_html(SPLASH)?;
    window.show();
    paths.log("startup screen shown");

    let mut app = App {
        view,
        paths: paths.clone(),
        settings: paths.load_settings(),
        downloads: Downloads::default(),
        connection: Connection::Browsing,
        target: Url::parse(YOUTUBE)?,
        attempts: 0,
        relay,
        invalid_routes,
        failed_routes: 0,
        last_playback_switch: None,
    };

    if app.invalid_routes {
        app.show_offline();
    } else {
        app.connect();
    }
    let mut scheduled = None;
    loop {
        let deadline = match app.connection {
            Connection::Loading { deadline, .. } => Some(deadline),
            Connection::Preparing { retry_at } => Some(retry_at),
            Connection::Offline { retry_at } => retry_at,
            Connection::Browsing => None,
        };
        if scheduled != deadline {
            window.schedule(deadline)?;
            scheduled = deadline;
        }
        match window.next_message()? {
            NativeMessage::Events => {
                for event in events.try_iter() {
                    app.handle(event);
                }
            }
            NativeMessage::Timer => {
                // Reschedule if Windows delivered the timer just before the deadline.
                scheduled = None;
                window.schedule(None)?;
                app.tick();
            }
            NativeMessage::Other => {}
            NativeMessage::Quit => return Ok(()),
        }
    }
}

#[derive(Clone)]
struct EventProxy {
    window: HWND,
    sender: mpsc::Sender<AppEvent>,
}

impl EventProxy {
    fn send(&self, event: AppEvent) {
        if self.sender.send(event).is_ok() {
            unsafe { PostMessageW(self.window, APP_EVENT, 0, 0) };
        }
    }
}

fn attach_events(webview: &ICoreWebView2, proxy: &EventProxy) -> windows::core::Result<()> {
    let starting = proxy.clone();
    let completed = proxy.clone();
    let messages = proxy.clone();
    unsafe {
        webview.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut id = 0;
                    let mut uri = PWSTR::null();
                    args.NavigationId(&mut id)?;
                    args.Uri(&mut uri)?;
                    let url = take_pwstr(uri);
                    starting.send(AppEvent::Starting { id, url });
                }
                Ok(())
            })),
            &mut 0,
        )?;
        webview.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut id = 0;
                    let mut success = BOOL::default();
                    args.NavigationId(&mut id)?;
                    args.IsSuccess(&mut success)?;
                    let result = if success.as_bool() {
                        Ok(())
                    } else {
                        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                        args.WebErrorStatus(&mut status)?;
                        Err(status.0)
                    };
                    completed.send(AppEvent::Completed { id, result });
                }
                Ok(())
            })),
            &mut 0,
        )?;
        webview.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut raw = PWSTR::null();
                    if args.TryGetWebMessageAsString(&mut raw).is_err() {
                        return Ok(());
                    }
                    let text = take_pwstr(raw);
                    if text.len() <= 512
                        && let Ok(message) = serde_json::from_str(&text)
                    {
                        let mut source = PWSTR::null();
                        args.Source(&mut source)?;
                        let source = take_pwstr(source);
                        messages.send(AppEvent::Message { source, message });
                    }
                }
                Ok(())
            })),
            &mut 0,
        )?;
    }
    Ok(())
}

impl App {
    fn handle(&mut self, event: AppEvent) {
        match event {
            AppEvent::Starting { id, url } => self.navigation_started(id, &url),
            AppEvent::Completed { id, result } => {
                if matches!(self.connection, Connection::Loading { id: Some(active), .. } if active == id)
                {
                    match result {
                        Ok(()) => {
                            self.paths.log("YouTube navigation completed");
                            self.connection = Connection::Browsing;
                            self.attempts = 0;
                            self.failed_routes = 0;
                        }
                        Err(status) => {
                            self.connection_failed(&format!("WebView2 network status {status}"))
                        }
                    }
                }
            }
            AppEvent::Message { source, message } => self.handle_message(&source, message),
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        match self.connection {
            Connection::Preparing { retry_at: at } | Connection::Offline { retry_at: Some(at) }
                if now >= at =>
            {
                self.connect()
            }
            Connection::Loading { deadline, .. } if now >= deadline => {
                let _ = unsafe { self.view.webview().Stop() };
                self.connection_failed("navigation timed out");
            }
            _ => {}
        }
    }

    fn connect(&mut self) {
        match self
            .relay
            .as_ref()
            .map_or(Readiness::Ready, Relay::readiness)
        {
            Readiness::Pending => {
                if !matches!(self.connection, Connection::Preparing { .. }) {
                    self.paths.log("Preparing Tor route");
                    let screen = SPLASH.replace("Загружаю YouTube…", "Подключаю резервный маршрут Tor…")
                        .replace("При сбое подключения приложение попробует следующий настроенный маршрут.",
                            "При первом подключении скачивается Tor Expert Bundle. Подключение может занять несколько минут.");
                    if let Err(error) = self.view.load_html(&screen) {
                        self.paths.log(format!("Tor status screen failed: {error}"));
                    }
                }
                self.connection = Connection::Preparing {
                    retry_at: Instant::now() + PREPARATION_POLL,
                };
                return;
            }
            Readiness::Failed => {
                self.connection_failed("Tor bootstrap failed");
                return;
            }
            Readiness::Ready => {}
        }
        self.attempts += 1;
        self.paths.log(format!(
            "YouTube connection attempt {} ({})",
            self.attempts,
            self.relay.as_ref().map_or("direct", Relay::country)
        ));
        self.connection = Connection::Loading {
            id: None,
            deadline: Instant::now() + LOAD_TIMEOUT,
        };
        if let Err(error) = self.view.load_url(self.target.as_str()) {
            self.connection_failed(&error.to_string());
        }
    }

    fn navigation_started(&mut self, id: u64, raw_url: &str) {
        if let Connection::Loading {
            id: Some(active), ..
        } = self.connection
            && active == id
        {
            return; // Redirects keep the original WebView2 navigation ID.
        }
        let Ok(url) = Url::parse(raw_url) else { return };
        if is_youtube(&url) {
            if let Connection::Loading {
                id: active @ None, ..
            } = &mut self.connection
            {
                *active = Some(id);
                return;
            }
            self.target = url;
            self.attempts = 1;
            self.connection = Connection::Loading {
                id: Some(id),
                deadline: Instant::now() + LOAD_TIMEOUT,
            };
        } else if url.as_str() != "about:blank" {
            self.connection = Connection::Browsing;
            self.attempts = 0;
        }
    }

    fn connection_failed(&mut self, reason: &str) {
        self.paths
            .log(format!("YouTube connection failed: {reason}"));
        if let Some(relay) = &self.relay
            && !self.downloads.state().active
        {
            self.failed_routes += 1;
            relay.advance();
            if self.failed_routes < relay.count() {
                self.connect();
                return;
            }
        }
        self.failed_routes = 0;
        self.show_offline();
    }

    fn show_offline(&mut self) {
        self.connection = Connection::Offline {
            retry_at: (!self.invalid_routes).then(|| Instant::now() + RETRY_DELAY),
        };
        let html = SPLASH
            .replace("data-offline=\"false\"", "data-offline=\"true\"")
            .replace(
                "data-attempts=\"0\"",
                &format!("data-attempts=\"{}\"", self.attempts),
            )
            .replace(
                "data-retry=\"0\"",
                &format!("data-retry=\"{}\"", RETRY_DELAY.as_secs()),
            );
        let html = if self.invalid_routes {
            html.replace("data-route-error=\"false\"", "data-route-error=\"true\"")
        } else {
            html
        };
        if let Err(error) = self.view.load_html(&html) {
            self.paths.log(format!("offline page failed: {error}"));
        }
    }

    fn handle_message(&mut self, source: &str, message: PageMessage) {
        if matches!(message, PageMessage::Retry | PageMessage::OpenRoutes) {
            if source == "about:blank"
                && matches!(self.connection, Connection::Offline { .. })
                && self.view.url().is_ok_and(|url| url == "about:blank")
            {
                if matches!(message, PageMessage::OpenRoutes) {
                    if let Err(error) = self.paths.edit_routes() {
                        self.paths
                            .log(format!("could not open routes.json: {error}"));
                        message_box(
                            "Не удалось открыть routes.json. Папка настроек: %APPDATA%\\MaklyxaTube.",
                        );
                    }
                } else if !self.invalid_routes {
                    self.connect();
                }
            }
            return;
        }
        let Ok(url) = Url::parse(source) else { return };
        let current = self.view.url().ok().and_then(|url| Url::parse(&url).ok());
        if !is_youtube(&url) || current.as_ref() != Some(&url) {
            return;
        }

        match message {
            PageMessage::Ready => {
                self.post(&json!({
                    "type": "ready",
                    "settings": self.settings.page_settings(),
                    "download": self.downloads.state(),
                    "region": self.relay.as_ref().map(Relay::country),
                    "canSwitchRegion": self.relay.as_ref().is_some_and(|relay| relay.count() > 1),
                }));
            }
            PageMessage::SetSetting { key, value } => {
                let mut next = self.settings.clone();
                match key {
                    Setting::Adblock => next.adblock = value,
                    Setting::Sponsorblock => next.sponsorblock = value,
                }
                let saved = match self.paths.save_settings(&next) {
                    Ok(()) => {
                        self.settings = next;
                        true
                    }
                    Err(error) => {
                        self.paths.log(format!("settings save failed: {error}"));
                        false
                    }
                };
                self.post(&json!({ "type": "settings", "settings": self.settings.page_settings(), "saved": saved }));
            }
            PageMessage::Download { mode } => {
                let started = video_url(&url).is_some_and(|video| {
                    self.downloads.start(
                        self.paths.clone(),
                        &self.settings.download_dir,
                        video,
                        mode,
                        self.relay.as_ref().map(Relay::proxy_url),
                    )
                });
                self.post(&json!({ "type": "download_start", "started": started, "download": self.downloads.state() }));
            }
            PageMessage::DownloadState => {
                self.post(&json!({ "type": "download", "download": self.downloads.state() }))
            }
            PageMessage::PlaybackStalled { position } => self.switch_playback_route(url, position),
            PageMessage::Retry | PageMessage::OpenRoutes => {}
        }
    }

    fn switch_playback_route(&mut self, mut url: Url, position: u32) {
        let Some(relay) = self.relay.as_ref().filter(|relay| relay.count() > 1) else {
            return;
        };
        let now = Instant::now();
        if !matches!(self.connection, Connection::Browsing)
            || video_url(&url).is_none()
            || self.downloads.state().active
            || self
                .last_playback_switch
                .is_some_and(|last| now.duration_since(last) < PLAYBACK_SWITCH_INTERVAL)
        {
            return;
        }
        if position > 0 {
            let params: Vec<_> = url
                .query_pairs()
                .filter(|(key, _)| key != "t")
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            url.query_pairs_mut()
                .clear()
                .extend_pairs(params)
                .append_pair("t", &position.to_string());
        }
        relay.advance();
        self.target = url;
        self.last_playback_switch = Some(now);
        self.connect();
    }

    fn post(&self, value: &impl Serialize) {
        if let Ok(json) = serde_json::to_string(value) {
            let result = unsafe {
                self.view
                    .webview()
                    .PostWebMessageAsJson(&HSTRING::from(json))
            };
            if let Err(error) = result {
                self.paths.log(format!("web message failed: {error}"));
            }
        }
    }
}
