#![cfg(not(target_os = "emscripten"))]

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tiny_http::{Header, Response, Server};

struct Browser(Child);

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn chrome() -> PathBuf {
    if let Some(path) = std::env::var_os("BROWSER") {
        return path.into();
    }
    [
        "C:/Program Files/Google/Chrome/Application/chrome.exe",
        "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
    .expect("Chrome not found; set BROWSER to its executable")
}

#[test]
fn browser_tasks() {
    let binary = Path::new(env!("CARGO_BIN_FILE_BROWSER_TEST_BIN"));
    let javascript = std::fs::read(binary).expect("read browser test binary");
    let wasm = std::fs::read(binary.with_extension("wasm")).expect("read browser test Wasm");
    let server = Server::http("127.0.0.1:0").unwrap();
    let profile = tempfile::tempdir().unwrap();
    let mut command = Command::new(chrome());
    command
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-extensions",
        ])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg(format!("http://{}/", server.server_addr()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut browser = Browser(command.spawn().expect("launch Chrome"));
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        assert!(
            Instant::now() < deadline,
            "browser tests timed out after 45 seconds"
        );
        if let Some(status) = browser.0.try_wait().unwrap() {
            panic!("Chrome exited before reporting test results: {status}");
        }
        let Some(mut request) = server.recv_timeout(Duration::from_millis(100)).unwrap() else {
            continue;
        };
        let (status, mime, body) = match request.url() {
            "/" => (
                200,
                "text/html",
                include_bytes!("browser/index.html").to_vec(),
            ),
            "/test.js" => (200, "text/javascript", javascript.clone()),
            "/test.wasm" => (200, "application/wasm", wasm.clone()),
            "/bytes" => (200, "application/octet-stream", vec![0, 1, 2, 255]),
            "/empty" => (204, "text/plain", Vec::new()),
            "/echo" => {
                let mut body = Vec::new();
                request.as_reader().read_to_end(&mut body).unwrap();
                (200, "text/plain", body)
            }
            "/delay" => {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(100));
                    let _ = request.respond(Response::from_string("delayed"));
                });
                continue;
            }
            "/script.js" => (
                200,
                "text/javascript",
                b"globalThis.__async_script_loads = (globalThis.__async_script_loads || 0) + 1;"
                    .to_vec(),
            ),
            "/worker.js" => (
                200,
                "text/javascript",
                br#"
                onmessage = ({data: request}) => postMessage({
                    callbackId: request.callbackId, finalResponse: true, data: request.data
                });
            "#
                .to_vec(),
            ),
            "/report" => {
                let report: serde_json::Value =
                    serde_json::from_reader(request.as_reader()).unwrap();
                request.respond(Response::from_string("ok")).unwrap();
                let lines = report["lines"].as_array().expect("test output");
                let output = lines
                    .iter()
                    .map(|line| line.as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("\n");
                println!("{output}");
                assert_eq!(
                    report["code"].as_i64(),
                    Some(0),
                    "browser tests failed:\n{output}"
                );
                assert!(
                    lines.iter().any(|line| line == "browser tests passed"),
                    "test binary did not finish:\n{output}"
                );
                break;
            }
            _ => (404, "text/plain", b"not found".to_vec()),
        };
        let response = Response::from_data(body)
            .with_status_code(status)
            .with_header(Header::from_bytes("Content-Type", mime).unwrap())
            .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap());
        // Canceled downloads may close the connection before we respond.
        let _ = request.respond(response);
    }
}
