use std::{
    ffi::OsString,
    fs,
    io::{self, BufRead, BufReader},
    os::windows::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

use url::Url;

use crate::{
    model::{DownloadMode, DownloadState},
    platform::Paths,
    tools::DownloadTools,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Default)]
pub struct Downloads(Arc<Mutex<DownloadState>>);

impl Downloads {
    pub fn state(&self) -> DownloadState {
        self.0.lock().expect("download state").clone()
    }

    pub fn start(&self, paths: Paths, directory: &Path, url: Url, mode: DownloadMode) -> bool {
        {
            let mut state = self.0.lock().expect("download state");
            if state.active {
                return false;
            }
            *state = DownloadState {
                active: true,
                pct: -1,
                ..DownloadState::default()
            };
        }

        let directory = directory.to_owned();
        let state = Arc::clone(&self.0);
        let worker_paths = paths.clone();
        let spawned = thread::Builder::new()
            .name("video-download".into())
            .spawn(move || {
                let result = run_download(&worker_paths, &directory, url, mode, &state);
                finish(result, &state, &worker_paths);
            });
        if let Err(error) = spawned {
            finish(Err(error.into()), &self.0, &paths);
        }
        true
    }
}

type DownloadResult<T> = Result<T, Box<dyn std::error::Error>>;

fn finish(result: DownloadResult<String>, state: &Mutex<DownloadState>, paths: &Paths) {
    let message = {
        let mut state = state.lock().expect("download state");
        state.active = false;
        match result {
            Ok(title) => {
                state.pct = 100;
                state.title = title;
                format!("download ok: {}", state.title)
            }
            Err(error) => {
                state.error = error
                    .to_string()
                    .replace(['\r', '\n'], " ")
                    .chars()
                    .take(300)
                    .collect();
                format!("download failed: {}", state.error)
            }
        }
    };
    paths.log(message);
}

fn run_download(
    paths: &Paths,
    directory: &Path,
    url: Url,
    mode: DownloadMode,
    state: &Mutex<DownloadState>,
) -> DownloadResult<String> {
    fs::create_dir_all(directory)?;
    let tools = DownloadTools::prepare(&paths.tools)
        .map_err(|error| io::Error::other(format!("Не удалось подготовить загрузчик: {error}")))?;
    let mut home = OsString::from("home:");
    home.push(directory);
    let mut runtime = OsString::from("deno:");
    runtime.push(&tools.deno);

    let mut command = Command::new(&tools.yt_dlp);
    command
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args([
            "--ignore-config",
            "--no-playlist",
            "--no-simulate",
            "--newline",
            "--no-colors",
            "--progress",
            "--progress-delta",
            "0.5",
            "--progress-template",
            "download:MT_PROGRESS:%(progress._percent_str)s",
            "--print",
            "before_dl:MT_TITLE:%(title)j",
            "--encoding",
            "utf-8",
            "--windows-filenames",
        ])
        .arg("--ffmpeg-location")
        .arg(&tools.ffmpeg_dir)
        .arg("--js-runtimes")
        .arg(runtime)
        .arg("--paths")
        .arg(home)
        .args(["--output", "%(title)s.%(ext)s"]);

    match mode {
        DownloadMode::Mp4 => command.args(["--format", "bv*+ba/b", "--merge-output-format", "mp4"]),
        DownloadMode::Mp3 => command.args([
            "--format",
            "ba/b",
            "--extract-audio",
            "--audio-format",
            "mp3",
        ]),
    };
    let mut child = command.arg("--").arg(url.as_str()).spawn()?;
    let stdout = child.stdout.take().expect("piped yt-dlp stdout");
    let stderr = child.stderr.take().expect("piped yt-dlp stderr");

    let (status, title, details) = thread::scope(|scope| {
        let error_reader = scope.spawn(move || {
            BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
                .filter(|line| !line.trim().is_empty())
                .last()
        });
        let mut title = None;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(value) = line.strip_prefix("MT_PROGRESS:") {
                let pct = value
                    .trim()
                    .trim_end_matches('%')
                    .parse::<f32>()
                    .map_or(-1, |pct| pct.clamp(0.0, 100.0) as i32);
                state.lock().expect("download state").pct = pct;
            }
            if let Some(value) = line.strip_prefix("MT_TITLE:") {
                title = serde_json::from_str::<String>(value).ok();
            }
        }
        (child.wait(), title, error_reader.join().unwrap_or_default())
    });

    let status = status?;
    if !status.success() {
        let details = details.unwrap_or_else(|| format!("yt-dlp завершился с кодом {status}"));
        return Err(io::Error::other(details).into());
    }
    Ok(title.unwrap_or_else(|| "видео".into()))
}
