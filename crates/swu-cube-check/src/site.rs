//! The site over REST: sign-in (authorization code + PKCE against the site's issuer, once, by a
//! human), a refresh token cached at `~/.config/dumpstar/swu-cube-check.json` (mode 0600), and a
//! [`Source`](crate::fetch::Source) that reads decks and searches from `/api/agent`. A token is
//! never printed, and an expired or rejected sign-in is an error naming `login`, never a silent
//! fallback.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::fetch::Source;

pub const DEFAULT_SITE: &str = "https://star.dumpster.cards";
const SCOPE: &str = "openid email offline_access";
const LOGIN_WAIT: Duration = Duration::from_secs(300);
const PAGE: u32 = 100;
const MAX_PAGES: usize = 100;
/// An access token this close to expiry is refreshed before use.
const SKEW: u64 = 30;

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Config {
    pub site: String,
    pub client_id: String,
    pub token_endpoint: String,
    pub refresh_token: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub access_expires_at: u64,
    /// Whose tenant this is, as the issuer named them at sign-in (display only).
    #[serde(default)]
    pub who: String,
}

pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("SWU_CUBE_CHECK_CONFIG") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config/dumpstar/swu-cube-check.json")
}

pub fn load(path: &PathBuf) -> Result<Option<Config>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => serde_json::from_str(&t)
            .map(Some)
            .map_err(|e| format!("{}: {e}; run `swu-cube-check login` again", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

pub fn save(path: &PathBuf, cfg: &Config) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let err = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(err)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(err)?;
    f.write_all(serde_json::to_string_pretty(cfg).unwrap().as_bytes())
        .map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..=c.len() {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn random_token() -> Result<String, String> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| format!("no randomness: {e}"))?;
    Ok(b64url(&b))
}

/// The S256 challenge for a verifier.
pub fn challenge(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

/// Who a JWT names (email, else subject), read without verifying: display only, never trust.
fn who_of(jwt: &str) -> Option<String> {
    let claims: Value = serde_json::from_slice(&b64url_decode(jwt.split('.').nth(1)?)?).ok()?;
    ["email", "preferred_username", "sub"]
        .iter()
        .find_map(|k| claims[k].as_str().map(String::from))
}

fn http() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())
}

fn get_json(c: &Client, url: &str) -> Result<Value, String> {
    let r = c.get(url).send().map_err(|e| format!("{url}: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("{url}: HTTP {}", r.status()));
    }
    r.json().map_err(|e| format!("{url}: {e}"))
}

fn token_request(c: &Client, endpoint: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    let r = c
        .post(endpoint)
        .form(form)
        .send()
        .map_err(|e| format!("token endpoint: {e}"))?;
    let status = r.status();
    let body: Value = r.json().unwrap_or(Value::Null);
    if !status.is_success() {
        let why = body["error_description"]
            .as_str()
            .or(body["error"].as_str())
            .unwrap_or("refused");
        return Err(format!("token endpoint: HTTP {status}: {why}"));
    }
    Ok(body)
}

fn apply_tokens(cfg: &mut Config, t: &Value) -> Result<(), String> {
    cfg.access_token = t["access_token"]
        .as_str()
        .ok_or("token endpoint sent no access_token")?
        .into();
    cfg.access_expires_at = now() + t["expires_in"].as_u64().unwrap_or(300);
    if let Some(r) = t["refresh_token"].as_str() {
        cfg.refresh_token = r.into();
    }
    if let Some(w) = t["id_token"]
        .as_str()
        .and_then(who_of)
        .or_else(|| who_of(&cfg.access_token))
    {
        cfg.who = w;
    }
    Ok(())
}

/// Sign in through a browser: print the authorization URL on stderr, catch the redirect on a
/// loopback port, and cache the refresh token. `announce` receives the URL.
pub fn login(
    path: &PathBuf,
    site: &str,
    client_id: &str,
    announce: &dyn Fn(&str),
) -> Result<Config, String> {
    let site = site.trim_end_matches('/');
    let c = http()?;
    let resource = get_json(&c, &format!("{site}/.well-known/oauth-protected-resource"))?;
    let issuer = resource["authorization_servers"][0]
        .as_str()
        .ok_or("the site names no authorization server")?
        .trim_end_matches('/')
        .to_string();
    let meta = get_json(&c, &format!("{issuer}/.well-known/openid-configuration"))?;
    let auth_ep = meta["authorization_endpoint"]
        .as_str()
        .ok_or("issuer has no authorization_endpoint")?;
    let token_ep = meta["token_endpoint"]
        .as_str()
        .ok_or("issuer has no token_endpoint")?;

    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let redirect = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr().unwrap().port()
    );
    let (verifier, state) = (random_token()?, random_token()?);
    let mut url = reqwest::Url::parse(auth_ep).map_err(|e| e.to_string())?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("scope", SCOPE)
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge(&verifier))
            .append_pair("code_challenge_method", "S256");
        if let Some(r) = resource["resource"].as_str() {
            q.append_pair("resource", r);
        }
    }
    announce(url.as_str());

    let code = wait_for_code(&listener, &state)?;
    let t = token_request(
        &c,
        token_ep,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("client_id", client_id),
            ("code_verifier", &verifier),
        ],
    )?;
    let mut cfg = Config {
        site: site.into(),
        client_id: client_id.into(),
        token_endpoint: token_ep.into(),
        ..Default::default()
    };
    apply_tokens(&mut cfg, &t)?;
    if t["refresh_token"].as_str().is_none() {
        return Err("the issuer sent no refresh token (offline_access refused)".into());
    }
    save(path, &cfg)?;
    Ok(cfg)
}

fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + LOGIN_WAIT;
    loop {
        match listener.accept() {
            Ok((mut s, _)) => {
                s.set_nonblocking(false).ok();
                s.set_read_timeout(Some(Duration::from_secs(5))).ok();
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]);
                let target = head.split_whitespace().nth(1).unwrap_or("/");
                let Ok(u) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
                    continue;
                };
                if u.path() != "/callback" {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                    continue;
                }
                let q = |k: &str| {
                    u.query_pairs()
                        .find(|(n, _)| n == k)
                        .map(|(_, v)| v.into_owned())
                };
                let mut reply = |m: &str| {
                    let _ = write!(
                        s,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{m}",
                        m.len()
                    );
                };
                if q("state").as_deref() != Some(state) {
                    reply("Sign-in refused: state mismatch.");
                    return Err("sign-in redirect carried the wrong state".into());
                }
                if let Some(e) = q("error") {
                    reply("Sign-in refused.");
                    return Err(format!("sign-in refused: {e}"));
                }
                let Some(code) = q("code") else {
                    reply("Sign-in refused: no code.");
                    return Err("sign-in redirect carried no code".into());
                };
                reply("Signed in. You can close this tab.");
                return Ok(code);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() > deadline {
                    return Err("timed out waiting for the sign-in redirect".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

const RELOGIN: &str = "run `swu-cube-check login`";
const RECONNECT: &str = "reconnect dumpstar in Claude Code (/mcp) or run `swu-cube-check login`";

/// What Claude Code's own MCP sign-in holds for a site. Read-only: its store is never written and
/// its refresh token is never used (rotating it would break Claude Code's copy).
#[derive(Debug, PartialEq)]
pub enum Mcp {
    /// No readable entry for this site: not available, never an error.
    Absent,
    /// An entry whose access token has expired.
    Expired(String),
    Valid {
        server: String,
        token: String,
    },
}

fn origin(url: &str) -> Option<String> {
    let u = reqwest::Url::parse(url).ok()?;
    Some(format!(
        "{}://{}{}",
        u.scheme(),
        u.host_str()?,
        u.port().map(|p| format!(":{p}")).unwrap_or_default()
    ))
}

/// Pick the `mcpOAuth` entry in a Claude Code credentials document whose `serverUrl` origin is
/// `site`'s. `now_ms` is the clock in epoch milliseconds.
pub fn mcp_entry(creds: &str, site: &str, now_ms: u64) -> Mcp {
    let Ok(v) = serde_json::from_str::<Value>(creds) else {
        return Mcp::Absent;
    };
    let Some(want) = origin(site) else {
        return Mcp::Absent;
    };
    let mut expired = None;
    for (key, e) in v["mcpOAuth"].as_object().into_iter().flatten() {
        if e["serverUrl"].as_str().and_then(origin).as_deref() != Some(&want) {
            continue;
        }
        let server = e["serverName"]
            .as_str()
            .or_else(|| key.split('|').next())
            .unwrap_or("dumpstar")
            .to_string();
        let Some(token) = e["accessToken"].as_str().filter(|t| !t.is_empty()) else {
            continue;
        };
        match e["expiresAt"].as_u64() {
            Some(at) if at > now_ms + SKEW * 1000 => {
                return Mcp::Valid {
                    server,
                    token: token.into(),
                };
            }
            _ => expired = Some(server),
        }
    }
    expired.map_or(Mcp::Absent, Mcp::Expired)
}

/// The text of Claude Code's credentials store: `$SWU_CUBE_CHECK_CLAUDE_CREDENTIALS`, else
/// `~/.claude/.credentials.json`, else (macOS) the login Keychain's "Claude Code-credentials".
fn claude_credentials() -> Option<String> {
    if let Some(p) = std::env::var_os("SWU_CUBE_CHECK_CLAUDE_CREDENTIALS") {
        return std::fs::read_to_string(p).ok();
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if let Ok(t) = std::fs::read_to_string(home.join(".claude/.credentials.json")) {
        return Some(t);
    }
    if cfg!(target_os = "macos") {
        let o = std::process::Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .output()
            .ok()?;
        if o.status.success() {
            return String::from_utf8(o.stdout).ok();
        }
    }
    None
}

pub fn claude_mcp(site: &str) -> Mcp {
    claude_credentials().map_or(Mcp::Absent, |t| mcp_entry(&t, site, now() * 1000))
}

/// The site a command talks to: `--site`, else the cached login's, else `$DUMPSTAR_SITE`, else
/// the default.
pub fn site_url(path: &PathBuf, site_override: Option<&str>) -> String {
    site_override
        .map(String::from)
        .or_else(|| load(path).ok().flatten().map(|c| c.site))
        .or_else(|| std::env::var("DUMPSTAR_SITE").ok())
        .unwrap_or_else(|| DEFAULT_SITE.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// The credential in use.
pub struct Credential {
    /// The source, never the token.
    pub label: String,
    pub token: String,
    pub base: String,
    /// True when the token is Claude Code's, which refreshes it when it next uses the MCP server.
    pub claude: bool,
}

/// The credential in use, in order: Claude Code's MCP sign-in, then the checker's own login.
pub fn credential(path: &PathBuf, site_override: Option<&str>) -> Result<Credential, String> {
    let base = site_url(path, site_override);
    match claude_mcp(&base) {
        Mcp::Valid { server, token } => {
            return Ok(Credential {
                label: format!("Claude Code MCP sign-in ({server})"),
                token,
                base,
                claude: true,
            });
        }
        Mcp::Expired(server) => {
            eprintln!("Claude Code's sign-in for {server} has expired: {RECONNECT}")
        }
        Mcp::Absent => {}
    }
    let (token, cfg) = access_token(path, site_override)?;
    let who = if cfg.who.is_empty() {
        "(unnamed)"
    } else {
        &cfg.who
    };
    Ok(Credential {
        label: format!("swu-cube-check login ({who})"),
        token,
        base: cfg.site,
        claude: false,
    })
}

/// The cached sign-in's access token, refreshed when it is near expiry. A missing cache or a
/// refresh the issuer refuses is an error naming `login`; nothing else is tried.
pub fn access_token(
    path: &PathBuf,
    site_override: Option<&str>,
) -> Result<(String, Config), String> {
    let mut cfg = load(path)?.ok_or(format!("not signed in: {RELOGIN}"))?;
    if let Some(s) = site_override
        && s.trim_end_matches('/') != cfg.site
    {
        return Err(format!(
            "signed in to {}, not {s}: {RELOGIN} --site {s}",
            cfg.site
        ));
    }
    if cfg.access_token.is_empty() || cfg.access_expires_at <= now() + SKEW {
        let t = token_request(
            &http()?,
            &cfg.token_endpoint,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &cfg.refresh_token),
                ("client_id", &cfg.client_id),
            ],
        )
        .map_err(|e| format!("sign-in expired ({e}): {RELOGIN}"))?;
        apply_tokens(&mut cfg, &t)?;
        save(path, &cfg)?;
    }
    Ok((cfg.access_token.clone(), cfg))
}

pub struct Site {
    base: String,
    token: String,
    claude: bool,
    client: Client,
}

/// The server's reason for a 403: the `error` and `detail` of its JSON body, else the body text.
fn forbidden_reason(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => {
            let part = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(String::from);
            match (part("error"), part("detail")) {
                (Some(e), Some(d)) => format!("{e}: {d}"),
                (Some(x), None) | (None, Some(x)) => x,
                _ => body.trim().to_string(),
            }
        }
        Err(_) => body.trim().to_string(),
    }
}

impl Site {
    pub fn open(path: &PathBuf, site_override: Option<&str>) -> Result<Site, String> {
        Site::with(credential(path, site_override)?)
    }

    pub fn with(c: Credential) -> Result<Site, String> {
        Ok(Site {
            base: c.base,
            token: c.token,
            claude: c.claude,
            client: http()?,
        })
    }

    /// `GET /api/agent/me`: the tenant handle the token is served as.
    pub fn tenant(&self) -> Result<String, String> {
        let v = self.get("/api/agent/me", &[])?;
        v["tenant"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| "/api/agent/me: no tenant in the reply".to_string())
    }

    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, String> {
        let url = format!("{}{path}", self.base);
        let r = self
            .client
            .get(&url)
            .bearer_auth(&self.token)
            .query(query)
            .send()
            .map_err(|e| format!("{path}: {e}"))?;
        let status = r.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(if self.claude {
                format!(
                    "{path}: Claude Code's dumpstar token has expired: make any dumpstar MCP call in Claude Code to refresh it, then rerun (or {RELOGIN})"
                )
            } else {
                format!("{path}: sign-in expired or invalid (HTTP {status}): {RELOGIN}")
            });
        }
        let text = r.text().unwrap_or_default();
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(format!(
                "{path}: signed in, but not allowed (HTTP {status}): {}",
                forbidden_reason(&text)
            ));
        }
        if !status.is_success() {
            return Err(format!("{path}: HTTP {status}: {text}"));
        }
        serde_json::from_str(&text).map_err(|e| format!("{path}: not JSON: {e}"))
    }
}

impl Source for Site {
    fn list_group(&self, group: &str) -> Result<Vec<i64>, String> {
        let v = self.get("/api/agent/decks", &[("group", group.to_string())])?;
        let mut ids: Vec<i64> = v["decks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d["id"].as_i64())
            .collect();
        ids.sort();
        Ok(ids)
    }

    fn deck(&self, id: i64) -> Result<Value, String> {
        let v = self.get(&format!("/api/agent/decks/{id}"), &[])?;
        if v.get("lines_digest").is_none() {
            return Err(format!("deck {id}: the site sent no lines_digest"));
        }
        crate::verify_digest(&v).map_err(|e| format!("deck {id}: {e}"))?;
        Ok(v)
    }

    fn search(&self, q: &str) -> Result<Vec<Value>, String> {
        let mut rows = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = vec![
                ("q", q.to_string()),
                ("limit", PAGE.to_string()),
                ("owned_only", "true".into()),
            ];
            if let Some(c) = &cursor {
                query.push(("cursor", c.clone()));
            }
            let v = self.get("/api/agent/search", &query)?;
            rows.extend(v["results"].as_array().cloned().unwrap_or_default());
            match v["next_cursor"].as_str() {
                Some(c) if !c.is_empty() => cursor = Some(c.into()),
                _ => return Ok(rows),
            }
        }
        Err(format!("search '{q}' did not end after {MAX_PAGES} pages"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s256_challenge_matches_rfc7636_appendix_b() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    const NOW: u64 = 1_000_000_000_000;

    fn creds(entries: &[(&str, &str, u64)]) -> String {
        let m: serde_json::Map<String, Value> = entries
            .iter()
            .map(|(name, url, exp)| {
                (
                    format!("{name}|abc123"),
                    serde_json::json!({"serverName": name, "serverUrl": url,
                        "accessToken": "tok", "refreshToken": "r", "expiresAt": exp}),
                )
            })
            .collect();
        serde_json::json!({"mcpOAuth": m}).to_string()
    }

    #[test]
    fn a_valid_entry_for_the_site_is_used() {
        let c = creds(&[("dumpstar", "https://star.dumpster.cards/mcp", NOW + 600_000)]);
        assert_eq!(
            mcp_entry(&c, "https://star.dumpster.cards", NOW),
            Mcp::Valid {
                server: "dumpstar".into(),
                token: "tok".into()
            }
        );
    }

    #[test]
    fn an_expired_entry_is_named_not_used() {
        let c = creds(&[("dumpstar", "https://star.dumpster.cards/mcp", NOW - 1)]);
        assert_eq!(
            mcp_entry(&c, "https://star.dumpster.cards", NOW),
            Mcp::Expired("dumpstar".into())
        );
    }

    #[test]
    fn another_server_is_absent() {
        let c = creds(&[("other", "https://other.example/mcp", NOW + 600_000)]);
        assert_eq!(
            mcp_entry(&c, "https://star.dumpster.cards", NOW),
            Mcp::Absent
        );
    }

    #[test]
    fn a_valid_entry_beats_an_expired_one_for_the_same_site() {
        let c = creds(&[
            ("old", "https://star.dumpster.cards/mcp", NOW - 1),
            ("dumpstar", "https://star.dumpster.cards/mcp", NOW + 600_000),
        ]);
        assert!(matches!(
            mcp_entry(&c, "https://star.dumpster.cards", NOW),
            Mcp::Valid { .. }
        ));
    }

    #[test]
    fn malformed_stores_are_absent() {
        for c in [
            "",
            "not json",
            "{}",
            r#"{"mcpOAuth": 3}"#,
            r#"{"mcpOAuth": {"a|b": {"serverUrl": 1}}}"#,
        ] {
            assert_eq!(
                mcp_entry(c, "https://star.dumpster.cards", NOW),
                Mcp::Absent,
                "{c}"
            );
        }
    }

    #[test]
    fn base64url_round_trips() {
        for n in 0..8 {
            let b: Vec<u8> = (0..n).map(|i| i * 37).collect();
            assert_eq!(b64url_decode(&b64url(&b)).unwrap(), b);
        }
    }
}
