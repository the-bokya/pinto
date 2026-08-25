use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Deserialize;
use serde_json::{Map, Value};

use pinto::{browser, merge, options, render};

/// Ingest Frappe's chrome print-format input (rendered HTML + options) and produce the PDF.
#[derive(Parser)]
#[command(name = "pinto", about = "Frappe chrome-PDF generator, reimplemented in Rust")]
struct Cli {
    /// Rendered printview HTML file ("-" for stdin).
    #[arg(long)]
    html: String,

    /// Options/config JSON file (see InputConfig). Defaults to an empty config.
    #[arg(long)]
    options: Option<PathBuf>,

    /// Output PDF path ("-" for stdout).
    #[arg(long, default_value = "-")]
    out: String,

    /// Path to the headless_shell binary (must match Frappe's Chrome build).
    #[arg(long)]
    chrome_path: Option<String>,

    /// Attach to an already-running Chrome DevTools WebSocket instead of launching.
    #[arg(long)]
    devtools_url: Option<String>,

    /// Seconds to wait for Chrome to print its DevTools URL.
    #[arg(long, default_value_t = 15)]
    start_timeout: u64,

    /// Use the legacy Chrome/CDP backend instead of the built-in renderer.
    #[arg(long, default_value_t = false)]
    chrome: bool,

    /// Prototype the native renderer to this path and exit.
    #[arg(long)]
    proto: Option<String>,

    /// Merge-only diagnostic: compose these body/header/footer PDFs and exit (no Chrome).
    #[arg(long)]
    merge_body: Option<String>,
    #[arg(long)]
    merge_header: Option<String>,
    #[arg(long)]
    merge_footer: Option<String>,
    #[arg(long, default_value_t = false)]
    merge_dyn_header: bool,
    #[arg(long, default_value_t = false)]
    merge_dyn_footer: bool,
    #[arg(long, default_value_t = false)]
    merge_pd: bool,
}

/// The chrome-PDF input contract: the Frappe `options` dict plus ambient values that
/// Frappe would read from the DB/session (which a standalone binary must be given).
#[derive(Deserialize)]
struct InputConfig {
    #[serde(default)]
    options: Map<String, Value>,
    #[serde(default)]
    is_print_designer: bool,
    #[serde(default = "default_host")]
    host_url: String,
    sid: Option<String>,
    bench_sites_path: Option<String>,
    site_public_path: Option<String>,
    #[serde(default = "default_a4")]
    default_page_size: String,
    default_page_height: Option<String>,
    default_page_width: Option<String>,
}

impl Default for InputConfig {
    fn default() -> Self {
        // Match the serde field defaults (which only apply during deserialization).
        InputConfig {
            options: Map::new(),
            is_print_designer: false,
            host_url: default_host(),
            sid: None,
            bench_sites_path: None,
            site_public_path: None,
            default_page_size: default_a4(),
            default_page_height: None,
            default_page_width: None,
        }
    }
}

fn default_host() -> String {
    "http://localhost:8000/".into()
}

fn default_a4() -> String {
    "A4".into()
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(out) = &cli.proto {
        render::proto::render(out)?;
        return Ok(());
    }

    if let Some(body_path) = &cli.merge_body {
        let merged = merge::transform_pdf(merge::MergeInput {
            body: std::fs::read(body_path)?,
            header: cli.merge_header.as_ref().map(std::fs::read).transpose()?,
            footer: cli.merge_footer.as_ref().map(std::fs::read).transpose()?,
            is_header_dynamic: cli.merge_dyn_header,
            is_footer_dynamic: cli.merge_dyn_footer,
            is_print_designer: cli.merge_pd,
        })?;
        write_output(&cli.out, &merged)?;
        return Ok(());
    }

    let html = read_input(&cli.html).context("reading HTML")?;
    let config: InputConfig = match &cli.options {
        Some(path) => {
            let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            serde_json::from_str(&text).context("parsing options JSON")?
        }
        None => InputConfig::default(),
    };

    if cli.chrome && cli.chrome_path.is_none() && cli.devtools_url.is_none() {
        bail!("--chrome needs --chrome-path <headless_shell> or --devtools-url <ws://…>");
    }

    let password = options::opt_str(&config.options, "password");

    let job = browser::Job {
        html: String::from_utf8_lossy(&html).into_owned(),
        options: config.options,
        is_print_designer: config.is_print_designer,
        host_url: config.host_url,
        sid: config.sid,
        bench_sites_path: config.bench_sites_path,
        site_public_path: config.site_public_path,
        default_page_size: config.default_page_size,
        default_page_height: config.default_page_height,
        default_page_width: config.default_page_width,
        chrome_path: cli.chrome_path,
        devtools_url: cli.devtools_url,
        start_timeout: Duration::from_secs(cli.start_timeout),
    };

    let mut pdf = if cli.chrome {
        browser::run(job).await?
    } else {
        render::engine::render(&job)?
    };

    if let Some(password) = password {
        pdf = encrypt(&pdf, &password)?;
    }

    write_output(&cli.out, &pdf).context("writing PDF")?;
    Ok(())
}

fn read_input(path: &str) -> Result<Vec<u8>> {
    if path == "-" {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf)?;
        Ok(buf)
    } else {
        Ok(std::fs::read(path)?)
    }
}

fn write_output(path: &str, data: &[u8]) -> Result<()> {
    if path == "-" {
        std::io::stdout().write_all(data)?;
    } else {
        std::fs::write(path, data)?;
    }
    Ok(())
}

/// Password-protect the PDF with qpdf (RC4-128), matching pypdf's writer.encrypt intent.
/// Encryption output is inherently non-deterministic, so this is outside the identical-render goal.
fn encrypt(pdf: &[u8], password: &str) -> Result<Vec<u8>> {
    use std::process::Command;
    let dir = std::env::temp_dir();
    let input = dir.join(format!("pinto-{}.pdf", std::process::id()));
    let output = dir.join(format!("pinto-{}-enc.pdf", std::process::id()));
    std::fs::write(&input, pdf)?;
    let status = Command::new("qpdf")
        .args(["--encrypt", password, password, "128", "--", input.to_str().unwrap(), output.to_str().unwrap()])
        .status();
    let result = match status {
        Ok(s) if s.success() || s.code() == Some(3) => std::fs::read(&output).map_err(Into::into),
        Ok(_) => {
            eprintln!("pinto: qpdf encryption failed; emitting unencrypted PDF");
            Ok(pdf.to_vec())
        }
        Err(_) => {
            eprintln!("pinto: qpdf not found; emitting unencrypted PDF");
            Ok(pdf.to_vec())
        }
    };
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
    result
}
