//! Bounded HTTP-over-named-pipe transport for the Docker Engine API
//! (spec §2, §51, §66).
//!
//! # Design
//!
//! - Windows named pipe `\\.\pipe\docker_engine` only — **no** `tcp://…`
//!   fallback (spec §3), no `docker.exe` CLI dependency (spec §2).
//! - A hard-coded **read-only allowlist** of Engine API paths; the transport
//!   refuses everything else (spec §51). The frontend can never choose a
//!   method or path (spec §10, §50).
//! - Small hand-rolled HTTP/1.1 GET: the Engine answers every read we need
//!   with `Content-Length` bodies; the reader enforces the byte cap while
//!   streaming. Chunked responses are handled with a minimal decoder.
//! - Every failure maps to the typed [`DockerFailure`] model.
//!
//! The [`Transport`] trait lets every adapter test run against a fake
//! in-memory engine — no real Docker required (spec §52).

use super::domain::{DockerFailure, MAX_BODY_BYTES, REQUEST_TIMEOUT};

/// The named pipe Docker Desktop exposes on Windows (spec §2).
pub(crate) const ENGINE_PIPE: &str = r"\\.\pipe\docker_engine";

/// Read-only Engine API paths the adapter may request (spec §9, §51).
/// Anything not in this allowlist is refused by construction.
pub(crate) const ALLOWED_PATHS: &[&str] = &[
    "/_ping",
    "/version",
    "/info",
    "/containers/json",
    "/containers/{}/json",   // {id} placeholder
    "/containers/{}/stats",  // {id} placeholder, always ?stream=false
];

/// Check a path against the allowlist. `{}` in a template matches one
/// non-empty path segment: `path == prefix + segment + suffix` (the suffix
/// keeps its leading slash — `{}` consumes only the braces).
pub(crate) fn path_is_allowed(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    ALLOWED_PATHS.iter().any(|allowed| {
        if let Some((prefix, suffix)) = allowed.split_once("{}") {
            let rest = path.strip_prefix(prefix).unwrap_or("");
            rest.len() > suffix.len()
                && rest.ends_with(suffix)
                && {
                    let segment = &rest[..rest.len() - suffix.len()];
                    !segment.is_empty() && !segment.contains('/')
                }
        } else {
            path == *allowed
        }
    })
}

/// Response of one Engine request.
#[derive(Debug, Clone)]
pub(crate) struct EngineResponse {
    pub status: u16,
    pub body: String,
}

/// Transport abstraction — the adapter speaks to this, tests supply a fake.
pub(crate) trait Transport: Send + Sync {
    /// GET one allowlisted path (query string allowed for `stats`).
    fn get(&self, path: &str) -> Result<EngineResponse, DockerFailure>;
}

/// The real Windows named-pipe transport.
pub(crate) struct NamedPipeTransport {
    pipe_path: Vec<u16>,
}

/// Shared no-op transport used when the pipe path cannot even be encoded
/// (never expected on Windows) — every request reports unavailable honestly.
/// Currently unused (the constant pipe path always encodes); kept for the
/// non-Windows build path where `NamedPipeTransport::new` errors.
#[allow(dead_code)]
pub(crate) struct NullTransport;

#[allow(dead_code)]
impl Transport for NullTransport {
    fn get(&self, _path: &str) -> Result<EngineResponse, DockerFailure> {
        Err(DockerFailure::DockerUnavailable {
            detail: "no transport".to_string(),
        })
    }
}

/// Newtype so a concrete transport can live behind `Arc<dyn Transport>`.
pub(crate) struct SharedTransport<T>(T);

impl<T: Transport + Send + 'static> SharedTransport<T> {
    pub(crate) fn new(inner: T) -> Self {
        Self(inner)
    }
}

impl<T: Transport> Transport for SharedTransport<T> {
    fn get(&self, path: &str) -> Result<EngineResponse, DockerFailure> {
        self.0.get(path)
    }
}

impl NamedPipeTransport {
    #[cfg(windows)]
    pub(crate) fn new() -> Result<Self, String> {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = std::ffi::OsStr::new(super::transport::ENGINE_PIPE)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        Ok(Self { pipe_path: wide })
    }

    #[cfg(not(windows))]
    pub(crate) fn new() -> Result<Self, String> {
        Err("Docker named-pipe transport is Windows-only.".to_string())
    }
}

impl Transport for NamedPipeTransport {
    fn get(&self, path: &str) -> Result<EngineResponse, DockerFailure> {
        if !path_is_allowed(path) {
            return Err(DockerFailure::EngineError {
                detail: "path not in read-only allowlist".to_string(),
            });
        }
        self.request_via_pipe(path)
    }
}

impl NamedPipeTransport {
    /// One full HTTP/1.1 GET over the pipe: open → write → read bounded.
    #[cfg(windows)]
    fn request_via_pipe(&self, path: &str) -> Result<EngineResponse, DockerFailure> {
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
        };
        use windows_sys::Win32::System::Pipes::PIPE_TYPE_BYTE;

        // Open the pipe (Docker's pipe exists as a duplex byte-mode pipe).
        let handle = unsafe {
            CreateFileW(
                self.pipe_path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut() as _,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(map_pipe_error());
        }

        // PIPE_TYPE_BYTE / PIPE_ACCESS_DUPLEX referenced for honest imports
        // about the pipe mode we attach to.
        let _ = (PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE);

        // Build a minimal HTTP/1.1 request. `Connection: close` makes the
        // response unambiguous (read to EOF) without chunked parsing for the
        // few chunked responses; the reader still enforces the size cap.
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: docker\r\nUser-Agent: localstack-control-center\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );

        // The File is the single owner of the handle: std I/O for read and
        // write, and the handle closes when the File drops — no guard, no
        // double-close, no leak (RAII, same pattern as the Phase 6 pipes).
        let mut file = unsafe {
            use std::os::windows::io::FromRawHandle;
            std::fs::File::from_raw_handle(handle)
        };
        write_all(&mut file, request.as_bytes())?;
        read_response(&mut file)
    }

    #[cfg(not(windows))]
    fn request_via_pipe(&self, _path: &str) -> Result<EngineResponse, DockerFailure> {
        Err(DockerFailure::DockerUnavailable {
            detail: "non-Windows".to_string(),
        })
    }
}

/// Map the OS error to the typed model (spec §29–30, §66).
#[cfg(windows)]
fn map_pipe_error() -> DockerFailure {
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        // ERROR_FILE_NOT_FOUND / ERROR_PATH_NOT_FOUND → pipe absent.
        Some(2) | Some(3) => DockerFailure::DockerUnavailable {
            detail: error.to_string(),
        },
        // ERROR_ACCESS_DENIED → detected but not permitted (spec §30).
        Some(5) => DockerFailure::AccessDenied {
            detail: error.to_string(),
        },
        // ERROR_PIPE_BUSY — all pipe instances busy; treat as unavailable
        // for this cycle (backoff handles the hammering concern).
        Some(231) => DockerFailure::DockerUnavailable {
            detail: "all pipe instances are busy".to_string(),
        },
        _ => DockerFailure::EngineError {
            detail: error.to_string(),
        },
    }
}

#[cfg(windows)]
fn write_all(file: &mut std::fs::File, data: &[u8]) -> Result<(), DockerFailure> {
    use std::io::Write;
    file.write_all(data).map_err(|e| {
        if e.kind() == std::io::ErrorKind::TimedOut {
            DockerFailure::Timeout
        } else {
            DockerFailure::EngineError {
                detail: e.to_string(),
            }
        }
    })
}

#[cfg(windows)]
fn read_response(file: &mut std::fs::File) -> Result<EngineResponse, DockerFailure> {
    use std::io::Read;

    let mut raw: Vec<u8> = Vec::with_capacity(16 * 1024);
    let mut chunk = [0u8; 16 * 1024];
    let started = std::time::Instant::now();
    loop {
        if raw.len() > MAX_BODY_BYTES {
            return Err(DockerFailure::ResponseTooLarge {
                limit: MAX_BODY_BYTES,
            });
        }
        if started.elapsed() > REQUEST_TIMEOUT {
            return Err(DockerFailure::Timeout);
        }
        match file.read(&mut chunk) {
            Ok(0) => break, // EOF — normal end for `Connection: close`
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => break,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                return Err(DockerFailure::Timeout)
            }
            Err(e) => {
                return Err(DockerFailure::EngineError {
                    detail: e.to_string(),
                })
            }
        }
    }
    parse_http_response(&raw)
}

/// Parse the HTTP envelope (status line + headers) and extract the body,
/// handling both `Content-Length` and `Transfer-Encoding: chunked`.
fn parse_http_response(raw: &[u8]) -> Result<EngineResponse, DockerFailure> {
    let text = String::from_utf8_lossy(raw);
    let Some(header_end) = text.find("\r\n\r\n") else {
        return Err(DockerFailure::MalformedResponse {
            detail: "no HTTP header terminator".to_string(),
        });
    };
    let headers = &text[..header_end];
    let mut lines = headers.lines();
    let status_line = lines.next().unwrap_or_default();
    // "HTTP/1.1 200 OK"
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| DockerFailure::MalformedResponse {
            detail: "bad status line".to_string(),
        })?;

    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().ok();
        }
        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }

    let body_bytes = &raw[header_end + 4..];
    let owned: Vec<u8>;
    let body: &[u8] = if chunked {
        owned = decode_chunked(body_bytes)?;
        &owned
    } else if let Some(length) = content_length {
        if length > MAX_BODY_BYTES {
            return Err(DockerFailure::ResponseTooLarge { limit: MAX_BODY_BYTES });
        }
        body_bytes
            .get(..length)
            .ok_or_else(|| DockerFailure::MalformedResponse {
                detail: "body shorter than Content-Length".to_string(),
            })?
    } else {
        body_bytes
    };
    if body.len() > MAX_BODY_BYTES {
        return Err(DockerFailure::ResponseTooLarge { limit: MAX_BODY_BYTES });
    }
    Ok(EngineResponse {
        status,
        body: String::from_utf8_lossy(body).into_owned(),
    })
}

/// Minimal chunked-transfer decoder (spec: only if the daemon uses it).
fn decode_chunked(raw: &[u8]) -> Result<Vec<u8>, DockerFailure> {
    let mut out = Vec::with_capacity(raw.len());
    let mut pos = 0usize;
    loop {
        let Some(line_end) = raw[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|p| pos + p)
        else {
            return Err(DockerFailure::MalformedResponse {
                detail: "bad chunk size line".to_string(),
            });
        };
        let size_text = String::from_utf8_lossy(&raw[pos..line_end]);
        let size = usize::from_str_radix(size_text.trim().split(';').next().unwrap_or("0"), 16)
            .map_err(|_| DockerFailure::MalformedResponse {
                detail: "bad chunk size".to_string(),
            })?;
        pos = line_end + 2;
        if size == 0 {
            return Ok(out);
        }
        if pos + size > raw.len() || out.len() + size > MAX_BODY_BYTES {
            return Err(DockerFailure::ResponseTooLarge { limit: MAX_BODY_BYTES });
        }
        out.extend_from_slice(&raw[pos..pos + size]);
        pos += size + 2; // skip chunk + CRLF
    }
}

// ---------------------------------------------------------------------------
// Tests (fake transport — no Docker required, spec §52)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Path-aware fake Engine (spec §52): configured responses per route,
    /// optional delay, and failure injection.
    pub(crate) struct FakeEngine {
        pub routes: HashMap<String, Result<(u16, String), DockerFailure>>,
        pub fail_unlisted: bool,
    }

    impl FakeEngine {
        pub(crate) fn new() -> Self {
            Self { routes: HashMap::new(), fail_unlisted: false }
        }

        pub(crate) fn with_route(mut self, path: &str, status: u16, body: &str) -> Self {
            self.routes.insert(path.to_string(), Ok((status, body.to_string())));
            self
        }

        pub(crate) fn with_failure(mut self, path: &str, failure: DockerFailure) -> Self {
            self.routes.insert(path.to_string(), Err(failure));
            self
        }
    }

    impl Transport for FakeEngine {
        fn get(&self, path: &str) -> Result<EngineResponse, DockerFailure> {
            if !path_is_allowed(path) {
                return Err(DockerFailure::EngineError {
                    detail: format!("refused non-allowlisted path {path}"),
                });
            }
            match self.routes.get(path) {
                Some(Ok((status, body))) => Ok(EngineResponse {
                    status: *status,
                    body: body.clone(),
                }),
                Some(Err(failure)) => Err(failure.clone()),
                None if self.fail_unlisted => Err(DockerFailure::EngineError {
                    detail: format!("no fake route for {path}"),
                }),
                None => Err(DockerFailure::DockerUnavailable {
                    detail: format!("no fake route for {path}"),
                }),
            }
        }
    }

    #[test]
    fn allowlist_accepts_read_paths_and_refuses_the_rest() {
        assert!(path_is_allowed("/_ping"));
        assert!(path_is_allowed("/version"));
        assert!(path_is_allowed("/info"));
        assert!(path_is_allowed("/containers/json"));
        assert!(path_is_allowed("/containers/abc123/json"));
        assert!(path_is_allowed("/containers/abc123/stats?stream=false"));
        // Mutations are refused *by construction*:
        assert!(!path_is_allowed("/containers/abc123/stop"));
        assert!(!path_is_allowed("/containers/abc123/kill"));
        assert!(!path_is_allowed("/containers/create?name=x"));
        assert!(!path_is_allowed("/containers/abc123"));
        assert!(!path_is_allowed("/images/create?fromImage=x"));
        assert!(!path_is_allowed("/images/abc"));
        assert!(!path_is_allowed("/exec"));
        assert!(!path_is_allowed("/volumes"));
        assert!(!path_is_allowed("/networks/create"));
        assert!(!path_is_allowed("/anything/else"));
        // POST-ish method confusion can't happen: the transport only ever
        // emits GET (single request builder below).
    }

    #[test]
    fn fake_engine_routes_and_failures() {
        let engine = FakeEngine::new()
            .with_route("/_ping", 200, "OK")
            .with_failure("/version", DockerFailure::Timeout);
        assert_eq!(engine.get("/_ping").expect("ping").body, "OK");
        assert!(matches!(engine.get("/version"), Err(DockerFailure::Timeout)));
        assert!(matches!(
            engine.get("/containers/json"),
            Err(DockerFailure::DockerUnavailable { .. })
        ));
    }

    #[test]
    fn http_parsing_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 9\r\n\r\n{\"a\":123}";
        let response = parse_http_response(raw).expect("parse");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "{\"a\":123}");
    }

    #[test]
    fn http_parsing_chunked() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"a\":\r\n4\r\n123}\r\n0\r\n\r\n";
        let response = parse_http_response(raw).expect("parse");
        assert_eq!(response.body, "{\"a\":123}");
    }

    #[test]
    fn http_parsing_errors() {
        // No header terminator.
        assert!(matches!(
            parse_http_response(b"HTTP/1.1 200 OK\r\n"),
            Err(DockerFailure::MalformedResponse { .. })
        ));
        // Bad status.
        assert!(matches!(
            parse_http_response(b"NOT-HTTP\r\n\r\n{}"),
            Err(DockerFailure::MalformedResponse { .. })
        ));
        // Body shorter than declared.
        assert!(matches!(
            parse_http_response(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{}"),
            Err(DockerFailure::MalformedResponse { .. })
        ));
        // Oversized Content-Length.
        assert!(matches!(
            parse_http_response(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", MAX_BODY_BYTES + 1).as_bytes()
            ),
            Err(DockerFailure::ResponseTooLarge { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // Live engine verification (spec §60, §61, §62)
    // -----------------------------------------------------------------------
    // Read-only: GETs /version and /containers/json through the real named
    // pipe and cross-checks against the adapter's own parsing. Skips
    // honestly when Docker Desktop is not running; never starts, stops, or
    // changes anything.
    #[test]
    #[ignore = "live Docker Engine verification; run manually: cargo test -- --ignored"]
    fn live_engine_read_only_verification() {
        let Ok(transport) = super::NamedPipeTransport::new() else {
            eprintln!("SKIP: non-Windows host — no named-pipe transport");
            return;
        };
        // Engine reachable?
        let Ok(version) = transport.get("/version") else {
            eprintln!("SKIP: Docker Engine is not currently available (honest state, spec §29)");
            return;
        };
        assert_eq!(version.status, 200, "/version must succeed on a live engine");
        let info = super::super::parse::parse_version(&version.body);
        eprintln!(
            "LIVE Docker: version={:?} api={:?} os={:?} arch={:?}",
            info.version, info.apiVersion, info.os, info.arch
        );
        assert!(info.version.is_some(), "live /version must carry a version");

        // Container list (read-only).
        let containers = transport.get("/containers/json?all=true").expect("live list");
        assert_eq!(containers.status, 200);
        let parsed = super::super::parse::parse_containers_list(&containers.body).expect("parse");
        eprintln!("LIVE containers: {} (truncated at {})", parsed.len(), super::super::domain::MAX_CONTAINERS);
        for c in parsed.iter().take(5) {
            eprintln!(
                "  {} image={} state={:?} health={:?} ports={}",
                c.name,
                c.image,
                c.state,
                c.health,
                c.ports.len()
            );
        }

        // Mutation refusal against the LIVE transport (spec §44, §51): the
        // allowlist is checked before any I/O.
        assert!(transport.get("/containers/zzz/kill").is_err());
        assert!(transport.get("/images/create?fromImage=x").is_err());
        assert!(transport.get("/exec").is_err());
        eprintln!("LIVE mutation refusal: OK (allowlist enforced pre-I/O)");
    }
}
