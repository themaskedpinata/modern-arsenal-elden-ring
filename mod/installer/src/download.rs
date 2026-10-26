//! The pinned download (user decision D1: official sources only, each sha256-pinned, with an offline "Browse"
//! alternative). Only URLs in [`ALLOWED`] are fetched, only over https, into `<dest>.part`; the file is renamed to
//! its final name only when its sha256 equals the pin, otherwise it is deleted. Uses Windows TLS (schannel).

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::guard::Guard;
use crate::release::{PinnedSource, WITCHY_URL};
use crate::util;

/// The only URLs this program downloads from.
pub const ALLOWED: &[&str] = &[WITCHY_URL];

#[derive(Debug, PartialEq)]
pub enum DownloadError {
    NotAllowed(String),
    BadPin,
    Network(String),
    Io(String),
    HashMismatch { got: String, want: String },
    /// The player cancelled; the partial file was deleted.
    Cancelled,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::NotAllowed(u) => write!(f, "{u} is not a pinned official source: refusing to download"),
            DownloadError::BadPin => f.write_str("no valid sha256 pin: refusing to download"),
            DownloadError::Network(e) => write!(f, "download failed: {e}"),
            DownloadError::Io(e) => write!(f, "cannot write the download: {e}"),
            DownloadError::HashMismatch { got, want } => write!(
                f,
                "the downloaded file has sha256 {}, not the pinned {}: deleted, not used",
                util::short(got),
                util::short(want)
            ),
            DownloadError::Cancelled => f.write_str("cancelled"),
        }
    }
}

fn allowed(url: &str) -> bool {
    if ALLOWED.contains(&url) {
        return true;
    }
    cfg!(test) && url.starts_with("http://127.0.0.1:")
}

/// Downloads `src` to `dest` (through `dest.part`), reporting `(bytes, total)` progress. Returns the sha256.
pub fn fetch_pinned(src: &PinnedSource, dest: &Path, guard: &Guard, progress: impl FnMut(u64, Option<u64>)) -> Result<String, DownloadError> {
    fetch_pinned_cancellable(src, dest, guard, &AtomicBool::new(false), progress)
}

/// [`fetch_pinned`], stopped between two 64 KB reads when `cancel` becomes true (the `.part` file is deleted).
pub fn fetch_pinned_cancellable(
    src: &PinnedSource,
    dest: &Path,
    guard: &Guard,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<String, DownloadError> {
    if !allowed(&src.url) {
        return Err(DownloadError::NotAllowed(src.url.clone()));
    }
    if !util::is_sha256(&src.sha256) {
        return Err(DownloadError::BadPin);
    }
    let want = src.sha256.to_lowercase();
    let part = dest.with_file_name(format!("{}.part", dest.file_name().and_then(|n| n.to_str()).unwrap_or("download")));
    guard.check(dest).map_err(|e| DownloadError::Io(e.to_string()))?;
    guard.check(&part).map_err(|e| DownloadError::Io(e.to_string()))?;
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| DownloadError::Io(e.to_string()))?;
    }
    let _ = std::fs::remove_file(&part);
    let result = (|| {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(!cfg!(test))
            .user_agent("MW2inER-setup")
            .timeout_connect(Some(std::time::Duration::from_secs(30)))
            .build()
            .into();
        let resp = agent.get(&src.url).call().map_err(|e| DownloadError::Network(e.to_string()))?;
        let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
        let mut body = resp.into_body();
        let mut r = body.with_config().limit(512 * 1024 * 1024).reader();
        let mut f = std::fs::File::create(&part).map_err(|e| DownloadError::Io(e.to_string()))?;
        let mut buf = vec![0u8; 1 << 16];
        let mut n = 0u64;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(DownloadError::Cancelled);
            }
            let k = r.read(&mut buf).map_err(|e| DownloadError::Network(e.to_string()))?;
            if k == 0 {
                break;
            }
            f.write_all(&buf[..k]).map_err(|e| DownloadError::Io(e.to_string()))?;
            n += k as u64;
            progress(n, total);
        }
        f.sync_all().map_err(|e| DownloadError::Io(e.to_string()))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let got = util::sha256_file(&part).map_err(|e| DownloadError::Io(e.to_string()))?;
    if got != want {
        let _ = std::fs::remove_file(&part);
        return Err(DownloadError::HashMismatch { got, want });
    }
    let _ = std::fs::remove_file(dest);
    std::fs::rename(&part, dest).map_err(|e| DownloadError::Io(e.to_string()))?;
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    /// A one-shot local HTTP server (127.0.0.1 only; tests never reach the internet).
    fn serve(body: &'static [u8]) -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut req = [0u8; 2048];
                let _ = s.read(&mut req);
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(body);
            }
        });
        format!("http://127.0.0.1:{port}/w.zip")
    }

    #[test]
    fn refuses_unpinned_and_unlisted_sources() {
        let t = TempDir::new("dl-refuse");
        let g = Guard::default();
        let d = t.0.join("w.zip");
        let src = PinnedSource { url: "https://example.com/w.zip".into(), sha256: util::sha256_bytes(b"x"), file_name: "w.zip".into() };
        assert!(matches!(fetch_pinned(&src, &d, &g, |_, _| {}), Err(DownloadError::NotAllowed(_))));
        let src = PinnedSource { url: WITCHY_URL.into(), sha256: "abc".into(), file_name: "w.zip".into() };
        assert_eq!(fetch_pinned(&src, &d, &g, |_, _| {}), Err(DownloadError::BadPin));
        assert!(!d.exists());
    }

    #[test]
    fn keeps_only_a_matching_file() {
        let t = TempDir::new("dl");
        let g = Guard::default();
        let d = t.0.join(r"downloads\w.zip");
        let good = PinnedSource { url: serve(b"zip bytes"), sha256: util::sha256_bytes(b"zip bytes"), file_name: "w.zip".into() };
        let mut seen = 0;
        let h = fetch_pinned(&good, &d, &g, |n, _| seen = n).unwrap();
        assert_eq!(h, util::sha256_bytes(b"zip bytes"));
        assert_eq!(seen, 9);
        assert_eq!(std::fs::read(&d).unwrap(), b"zip bytes");
        let bad = PinnedSource { url: serve(b"tampered"), sha256: util::sha256_bytes(b"zip bytes"), file_name: "w.zip".into() };
        let d2 = t.0.join(r"downloads\w2.zip");
        assert!(matches!(fetch_pinned(&bad, &d2, &g, |_, _| {}), Err(DownloadError::HashMismatch { .. })));
        assert!(!d2.exists());
        assert_eq!(std::fs::read_dir(t.0.join("downloads")).unwrap().count(), 1, "no .part left");
        // unreachable source
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/x", l.local_addr().unwrap().port());
        drop(l);
        let gone = PinnedSource { url, sha256: util::sha256_bytes(b"x"), file_name: "x".into() };
        assert!(matches!(fetch_pinned(&gone, &t.0.join("x"), &g, |_, _| {}), Err(DownloadError::Network(_))));
        // cancelled: nothing kept
        let c = PinnedSource { url: serve(b"zip bytes"), sha256: util::sha256_bytes(b"zip bytes"), file_name: "w.zip".into() };
        let d4 = t.0.join(r"downloads\w4.zip");
        assert_eq!(fetch_pinned_cancellable(&c, &d4, &g, &AtomicBool::new(true), |_, _| {}), Err(DownloadError::Cancelled));
        assert!(!d4.exists() && !t.0.join(r"downloads\w4.zip.part").exists());
        // the guard applies
        let g2 = Guard::new(Some(&t.0.join("downloads")), None, &[], None, None);
        assert!(matches!(fetch_pinned(&good, &t.0.join(r"downloads\w3.zip"), &g2, |_, _| {}), Err(DownloadError::Io(_))));
    }
}
