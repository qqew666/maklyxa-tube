use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use ureq::{
    Agent,
    tls::{RootCerts, TlsConfig, TlsProvider},
};
use zip::ZipArchive;

type ToolResult<T> = Result<T, Box<dyn std::error::Error>>;

struct Asset {
    url: &'static str,
    sha256: &'static str,
    max_bytes: u64,
}

const YT_DLP: Asset = Asset {
    url: "https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp.exe",
    sha256: "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d",
    max_bytes: 30 * 1024 * 1024,
};
const FFMPEG: Asset = Asset {
    url: "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip",
    sha256: "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba",
    max_bytes: 130 * 1024 * 1024,
};
const DENO: Asset = Asset {
    url: "https://github.com/denoland/deno/releases/download/v2.9.8/deno-x86_64-pc-windows-msvc.zip",
    sha256: "cba84b26e0df5ae7f74fc66b86a95b866872a86292ebd84fa1e959927edd93bf",
    max_bytes: 60 * 1024 * 1024,
};

pub struct DownloadTools {
    pub yt_dlp: PathBuf,
    pub ffmpeg_dir: PathBuf,
    pub deno: PathBuf,
}

impl DownloadTools {
    pub fn prepare(cache: &Path) -> ToolResult<Self> {
        let yt_dlp = cache.join("yt-dlp-2026.10.26").join("yt-dlp.exe");
        let ffmpeg_dir = cache.join("ffmpeg-9.0.2");
        let ffmpeg = ffmpeg_dir.join("ffmpeg.exe");
        let ffprobe = ffmpeg_dir.join("ffprobe.exe");
        let deno = cache.join("deno-2.9.8").join("deno.exe");

        let agent: Agent = Agent::config_builder()
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .timeout_global(Some(Duration::from_secs(900)))
            .timeout_recv_response(Some(Duration::from_secs(40)))
            .build()
            .into();

        if !yt_dlp.is_file() {
            let parent = yt_dlp.parent().expect("yt-dlp cache directory");
            fetch(&agent, &YT_DLP, parent)?.persist(&yt_dlp)?;
        }
        if !ffmpeg.is_file() || !ffprobe.is_file() {
            let archive = fetch(&agent, &FFMPEG, &ffmpeg_dir)?;
            let mut zip = ZipArchive::new(archive.reopen()?)?;
            unpack(
                &mut zip,
                "ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe",
                &ffmpeg,
            )?;
            unpack(
                &mut zip,
                "ffmpeg-9.0.2-essentials_build/bin/ffprobe.exe",
                &ffprobe,
            )?;
        }
        if !deno.is_file() {
            let parent = deno.parent().expect("Deno cache directory");
            let archive = fetch(&agent, &DENO, parent)?;
            let mut zip = ZipArchive::new(archive.reopen()?)?;
            unpack(&mut zip, "deno.exe", &deno)?;
        }

        Ok(Self {
            yt_dlp,
            ffmpeg_dir,
            deno,
        })
    }
}

fn fetch(agent: &Agent, asset: &Asset, directory: &Path) -> ToolResult<NamedTempFile> {
    fs::create_dir_all(directory)?;
    let mut file = NamedTempFile::new_in(directory)?;
    let mut response = agent.get(asset.url).call()?;
    let mut body = response.body_mut().as_reader().take(asset.max_bytes + 1);
    let mut sha = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut total = 0;
    loop {
        let count = body.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > asset.max_bytes {
            return Err(io::Error::other("Размер скачанного файла превышает ожидаемый").into());
        }
        sha.update(&buffer[..count]);
        file.write_all(&buffer[..count])?;
    }
    if format!("{:x}", sha.finalize()) != asset.sha256 {
        return Err(io::Error::other("Контрольная сумма скачанного файла не совпадает").into());
    }
    Ok(file)
}

fn unpack(zip: &mut ZipArchive<File>, entry_name: &str, path: &Path) -> ToolResult<()> {
    if path.is_file() {
        return Ok(());
    }
    let mut entry = zip.by_name(entry_name)?;
    let mut file = NamedTempFile::new_in(path.parent().expect("tool cache directory"))?;
    io::copy(&mut entry, &mut file)?;
    file.persist(path)?;
    Ok(())
}
