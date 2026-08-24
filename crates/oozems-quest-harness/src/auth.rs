use std::fs;
use std::fs::OpenOptions;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use directories::BaseDirs;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use url::Url;

const OPENROUTER_ORIGIN: &str = "https://openrouter.ai";
const CALLBACK_PATH: &str = "/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAXIMUM_REQUEST_LINE_BYTES: u64 = 16 * 1024;

pub fn credential_path() -> Result<PathBuf> {
    let base = BaseDirs::new().context("could not locate the user configuration directory")?;
    Ok(base
        .config_dir()
        .join("oozems")
        .join("quest-harness")
        .join("openrouter.key"))
}

pub fn load_key(path: &Path) -> Result<Option<String>> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read credential {}", path.display()));
        }
    };
    let key = source.trim();
    if key.is_empty() {
        bail!("stored credential {} is empty", path.display());
    }
    Ok(Some(key.to_owned()))
}

pub fn save_key(
    path: &Path,
    key: &str,
) -> Result<()> {
    if key.trim() != key || key.is_empty() {
        bail!("OpenRouter returned an invalid empty or padded API key");
    }
    let parent = path
        .parent()
        .context("credential path has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create credential directory {}", parent.display()))?;
    restrict_directory(parent)?;

    let temporary = parent.join(format!(".openrouter.key.{}.tmp", std::process::id()));
    let write_result = write_private_file(&temporary, key)
        .and_then(|()| fs::rename(&temporary, path).context("failed to install credential"));
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

pub fn remove_key(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("failed to remove credential {}", path.display()))
        }
    }
}

pub fn browser_login(client: &Client) -> Result<String> {
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).context("failed to bind the browser login callback")?;
    listener
        .set_nonblocking(true)
        .context("failed to configure the browser login callback")?;
    let port = listener
        .local_addr()
        .context("failed to read the browser login callback address")?
        .port();
    let callback_url = format!("http://localhost:{port}{CALLBACK_PATH}");
    let verifier = create_code_verifier()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let authorization_url = authorization_url(&callback_url, &challenge)?;

    eprintln!("Open this URL to authorize the harness:\n{authorization_url}");
    if let Err(error) = webbrowser::open(authorization_url.as_str()) {
        eprintln!("The browser could not be opened automatically: {error}");
    }
    eprintln!("Waiting for the OpenRouter browser callback...");

    let code = wait_for_authorization_code(&listener, CALLBACK_TIMEOUT)?;
    exchange_code(client, &code, &verifier)
}

fn authorization_url(
    callback_url: &str,
    challenge: &str,
) -> Result<Url> {
    let mut url = Url::parse(&format!("{OPENROUTER_ORIGIN}/auth"))?;
    url.query_pairs_mut()
        .append_pair("callback_url", callback_url)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("key_label", "Oozems Quest Harness");
    Ok(url)
}

fn create_code_verifier() -> Result<String> {
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random).context("failed to obtain randomness for browser login")?;
    Ok(URL_SAFE_NO_PAD.encode(random))
}

fn wait_for_authorization_code(
    listener: &TcpListener,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => match read_callback_target(&mut stream) {
                Ok(target) => {
                    let result = parse_callback_target(&target);
                    write_browser_response(&mut stream, result.is_ok())?;
                    return result;
                }
                Err(error) => {
                    write_browser_response(&mut stream, false)?;
                    return Err(error);
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    bail!("OpenRouter browser login timed out after 10 minutes");
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error).context("browser login callback failed"),
        }
    }
}

fn read_callback_target(stream: &mut TcpStream) -> Result<String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .context("failed to configure the browser callback connection")?;
    let mut request_line = String::new();
    let bytes = BufReader::new(stream)
        .take(MAXIMUM_REQUEST_LINE_BYTES)
        .read_line(&mut request_line)
        .context("failed to read the browser callback request")?;
    if bytes == 0 || !request_line.ends_with('\n') {
        bail!("browser callback request line is empty or too long");
    }
    let mut parts = request_line.split_ascii_whitespace();
    let method = parts.next();
    let target = parts.next();
    let version = parts.next();
    if method != Some("GET") || version.is_none() || parts.next().is_some() {
        bail!("browser callback is not a valid HTTP GET request");
    }
    target
        .map(str::to_owned)
        .context("browser callback has no request target")
}

fn parse_callback_target(target: &str) -> Result<String> {
    let url = Url::parse(&format!("http://localhost{target}"))
        .context("browser callback URL is invalid")?;
    if url.path() != CALLBACK_PATH {
        bail!("browser callback used an unexpected path");
    }
    let mut code = None;
    let mut oauth_error = None;
    let mut oauth_description = None;
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "code" if code.is_none() => code = Some(value.into_owned()),
            "error" if oauth_error.is_none() => oauth_error = Some(value.into_owned()),
            "error_description" if oauth_description.is_none() => {
                oauth_description = Some(value.into_owned());
            }
            _ => {}
        }
    }
    if let Some(error) = oauth_error {
        let description = oauth_description.unwrap_or_else(|| "no description".to_owned());
        bail!("OpenRouter authorization failed: {error}: {description}");
    }
    let code = code.context("browser callback did not contain an authorization code")?;
    if code.is_empty() {
        bail!("browser callback contained an empty authorization code");
    }
    Ok(code)
}

fn write_browser_response(
    stream: &mut TcpStream,
    success: bool,
) -> Result<()> {
    let (status, body) = if success {
        (
            "200 OK",
            "OpenRouter authorization received. You can close this tab and return to the terminal.",
        )
    } else {
        (
            "400 Bad Request",
            "OpenRouter authorization failed. Return to the terminal for details.",
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: \
         {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .context("failed to answer the browser callback")
}

#[derive(Serialize)]
struct KeyExchangeRequest<'a> {
    code: &'a str,
    code_verifier: &'a str,
    code_challenge_method: &'static str,
}

#[derive(Deserialize)]
struct KeyExchangeResponse {
    key: String,
}

fn exchange_code(
    client: &Client,
    code: &str,
    verifier: &str,
) -> Result<String> {
    let response = client
        .post(format!("{OPENROUTER_ORIGIN}/api/v1/auth/keys"))
        .json(&KeyExchangeRequest {
            code,
            code_verifier: verifier,
            code_challenge_method: "S256",
        })
        .send()
        .context("failed to exchange the OpenRouter authorization code")?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "response body could not be read".to_owned());
        bail!(
            "OpenRouter authorization code exchange returned {status}: {}",
            truncate(&body, 2_000)
        );
    }
    let exchange = response
        .json::<KeyExchangeResponse>()
        .context("OpenRouter returned an invalid authorization response")?;
    if exchange.key.trim() != exchange.key || exchange.key.is_empty() {
        bail!("OpenRouter returned an invalid API key");
    }
    Ok(exchange.key)
}

fn truncate(
    value: &str,
    maximum_chars: usize,
) -> String {
    value.chars().take(maximum_chars).collect()
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to restrict directory {}", path.display()))
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn write_private_file(
    path: &Path,
    key: &str,
) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("failed to create credential {}", path.display()))?;
    file.write_all(key.as_bytes())
        .with_context(|| format!("failed to write credential {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync credential {}", path.display()))
}

#[cfg(not(unix))]
fn write_private_file(
    path: &Path,
    key: &str,
) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("failed to create credential {}", path.display()))?;
    file.write_all(key.as_bytes())
        .with_context(|| format!("failed to write credential {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync credential {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_extracts_code_and_reports_oauth_errors() {
        assert_eq!(
            parse_callback_target("/callback?code=abc%2F123&ignored=x")
                .expect("authorization code"),
            "abc/123"
        );

        let error = parse_callback_target(
            "/callback?error=access_denied&error_description=The+user+declined",
        )
        .expect_err("OAuth error");
        assert!(error.to_string().contains("The user declined"));
        assert!(parse_callback_target("/other?code=x").is_err());
        assert!(parse_callback_target("/callback").is_err());
    }

    #[test]
    fn stored_key_round_trips_and_can_be_removed() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("nested/openrouter.key");

        assert!(load_key(&path).expect("missing key").is_none());
        save_key(&path, "sk-test").expect("save key");
        assert_eq!(
            load_key(&path).expect("load key").as_deref(),
            Some("sk-test")
        );
        assert!(remove_key(&path).expect("remove key"));
        assert!(!remove_key(&path).expect("already removed key"));
    }

    #[cfg(unix)]
    #[test]
    fn stored_key_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("credentials/openrouter.key");
        save_key(&path, "sk-test").expect("save key");

        assert_eq!(
            fs::metadata(path)
                .expect("credential metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
