//! Launch headless Chrome-for-Testing and discover its DevTools WebSocket URL.
//! Ports frappe/utils/pdf_generator/chrome_pdf_generator.py (production flag set).

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

/// The exact production switches Frappe passes to headless_shell.
fn chrome_flags() -> Vec<&'static str> {
    vec![
        "--remote-debugging-port=0",
        "--disable-gpu",
        "--disable-field-trial-config",
        "--disable-background-networking",
        "--disable-background-timer-throttling",
        "--disable-backgrounding-occluded-windows",
        "--disable-back-forward-cache",
        "--disable-breakpad",
        "--disable-client-side-phishing-detection",
        "--disable-component-extensions-with-background-pages",
        "--disable-component-update",
        "--no-default-browser-check",
        "--disable-default-apps",
        "--disable-dev-shm-usage",
        "--disable-extensions",
        "--disable-features=ImprovedCookieControls,LazyFrameLoading,GlobalMediaControls,DestroyProfileOnBrowserClose,MediaRouter,DialMediaRouteProvider,AcceptCHFrame,AutoExpandDetailsElement,CertificateTransparencyComponentUpdater,AvoidUnnecessaryBeforeUnloadCheckSync,Translate,HttpsUpgrades,PaintHolding,ThirdPartyStoragePartitioning,LensOverlay,PlzDedicatedWorker",
        "--allow-pre-commit-input",
        "--disable-hang-monitor",
        "--disable-ipc-flooding-protection",
        "--disable-popup-blocking",
        "--disable-prompt-on-repost",
        "--disable-renderer-backgrounding",
        "--force-color-profile=srgb",
        "--metrics-recording-only",
        "--no-first-run",
        "--password-store=basic",
        "--use-mock-keychain",
        "--no-service-autorun",
        "--export-tagged-pdf",
        "--disable-search-engine-choice-screen",
        "--unsafely-disable-devtools-self-xss-warnings",
        "--enable-use-zoom-for-dsf=false",
        "--use-angle",
        "--headless",
        "--hide-scrollbars",
        "--mute-audio",
        "--blink-settings=primaryHoverType=2,availableHoverTypes=2,primaryPointerType=4,availablePointerTypes=4",
        "--no-sandbox",
        "--no-startup-window",
    ]
}

pub struct Chromium {
    /// Held so the process is killed on drop (kill_on_drop); not read directly.
    #[allow(dead_code)]
    child: Child,
    pub devtools_url: String,
}

impl Chromium {
    pub async fn launch(chrome_path: &str, start_timeout: Duration) -> Result<Self> {
        let mut child = Command::new(chrome_path)
            .args(chrome_flags())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let stderr = child.stderr.take().expect("stderr piped");
        let devtools_url = timeout(start_timeout, scrape_devtools_url(stderr))
            .await
            .map_err(|_| anyhow::anyhow!("Chromium took too long to start"))??;

        Ok(Self { child, devtools_url })
    }
}

async fn scrape_devtools_url(stderr: tokio::process::ChildStderr) -> Result<String> {
    let mut lines = BufReader::new(stderr).lines();
    while let Some(line) = lines.next_line().await? {
        if line.contains("DevTools listening on")
            && let Some(pos) = line.find("ws://") {
                return Ok(line[pos..].trim().to_string());
            }
    }
    bail!("Chromium exited before printing a DevTools URL");
}
