//! The fixture dumpstar site the fetch-mode tests share.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

pub struct Fixture {
    pub base: String,
    pub seen: Arc<Mutex<Vec<String>>>,
}

/// A site that serves `decks` (group `swu-jumpstart-cube`), their draw cards as search rows,
/// and a token endpoint that grants `refresh_ok`.
pub fn serve(decks: Vec<Value>, refresh_ok: bool) -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (log, b) = (seen.clone(), base.clone());
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let (log, b, decks) = (log.clone(), b.clone(), decks.clone());
            std::thread::spawn(move || handle(s, &b, &decks, refresh_ok, &log));
        }
    });
    Fixture { base, seen }
}

fn respond(mut s: TcpStream, code: u16, body: &Value) {
    let body = body.to_string();
    let _ = write!(
        s,
        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn handle(
    mut s: TcpStream,
    base: &str,
    decks: &[Value],
    refresh_ok: bool,
    log: &Mutex<Vec<String>>,
) {
    let mut buf = vec![0u8; 16384];
    let mut n = 0;
    loop {
        let k = s.read(&mut buf[n..]).unwrap_or(0);
        n += k;
        if k == 0 || buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf[..n]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    let auth = head
        .lines()
        .find_map(|l| {
            l.strip_prefix("authorization: ")
                .or_else(|| l.strip_prefix("Authorization: "))
        })
        .unwrap_or("")
        .to_string();
    log.lock().unwrap().push(format!("{method} {target}"));
    let url = reqwest::Url::parse(&format!("http://x{target}")).unwrap();
    let q = |k: &str| {
        url.query_pairs()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.into_owned())
    };
    match (method, url.path()) {
        ("POST", "/token") if refresh_ok => respond(
            s,
            200,
            &json!({"access_token": "tok-fresh", "expires_in": 300, "refresh_token": "refresh-2"}),
        ),
        ("POST", "/token") => respond(s, 400, &json!({"error": "invalid_grant"})),
        ("GET", "/.well-known/oauth-protected-resource") => respond(
            s,
            200,
            &json!({"resource": format!("{base}/mcp"), "authorization_servers": [base]}),
        ),
        (_, p) if p.starts_with("/api/") && auth != "Bearer tok-fresh" => {
            respond(s, 401, &json!({}))
        }
        ("GET", "/api/agent/me") => respond(s, 200, &json!({"tenant": "concierge"})),
        ("GET", "/api/agent/decks") => {
            let list: Vec<Value> = decks
                .iter()
                .enumerate()
                .filter(|_| q("group").as_deref() == Some("swu-jumpstart-cube"))
                .map(|(i, d)| json!({"id": i + 1, "name": d["deck"]["name"], "group": "swu-jumpstart-cube"}))
                .collect();
            respond(s, 200, &json!({"decks": list}))
        }
        ("GET", p) if p.starts_with("/api/agent/decks/") => {
            let id: usize = p.rsplit('/').next().unwrap().parse().unwrap();
            match decks.get(id - 1) {
                Some(d) => {
                    let mut d = d.clone();
                    for c in d["cards"].as_object_mut().unwrap().values_mut() {
                        c["owned_count"] = json!(2);
                    }
                    d["lines_digest"] =
                        json!(swu_cube_check::lines_digest(&d["lines"], &d["cards"]));
                    if id == 7 && std::env::var_os("FIXTURE_TAMPER").is_some() {
                        d["cards"]["C7-0"]["cost"] = json!(9);
                    }
                    respond(s, 200, &d)
                }
                None => respond(s, 404, &json!({})),
            }
        }
        ("GET", "/api/agent/search") => {
            let mut rows = Vec::new();
            for d in decks {
                for (id, c) in d["cards"].as_object().unwrap() {
                    let query = q("q").unwrap_or_default();
                    if (id.starts_with('C') || query.starts_with('"')) && matches_query(c, &query) {
                        let mut r = c.clone();
                        r["id"] = json!(id);
                        r["owned_count"] = json!(2);
                        r["available_count"] = json!(1);
                        rows.push(r);
                    }
                }
            }
            let from: usize = q("cursor").and_then(|c| c.parse().ok()).unwrap_or(0);
            let limit: usize = q("limit").and_then(|c| c.parse().ok()).unwrap_or(25);
            assert!(limit <= 100);
            let end = (from + limit).min(rows.len());
            let next = (end < rows.len()).then(|| end.to_string());
            respond(
                s,
                200,
                &json!({"results": rows[from..end], "total": rows.len(), "next_cursor": next}),
            )
        }
        _ => respond(s, 404, &json!({})),
    }
}

/// The `a:`/`-a:` terms of a query against a card's aspects, the part of the language the pool
/// searches use.
fn matches_query(card: &Value, query: &str) -> bool {
    let aspects: Vec<String> = card["game"]["aspects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_lowercase())
        .collect();
    query
        .split_whitespace()
        .all(|t| match t.strip_prefix("-a:") {
            Some(a) => !aspects.iter().any(|x| x == a),
            None => t
                .strip_prefix("a:")
                .is_none_or(|a| aspects.iter().any(|x| x == a)),
        })
}

pub fn signed_in(
    dir: &std::path::Path,
    f: &Fixture,
    access: &str,
    expires_at: u64,
) -> std::path::PathBuf {
    let p = dir.join("swu-cube-check.json");
    std::fs::write(
        &p,
        json!({"site": f.base, "client_id": "cid", "token_endpoint": format!("{}/token", f.base),
               "refresh_token": "refresh-1", "access_token": access, "access_expires_at": expires_at,
               "who": "ryan@example.com"})
        .to_string(),
    )
    .unwrap();
    p
}

pub fn far_future() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600
}

pub fn claude_store(dir: &std::path::Path, f: &Fixture, expires_ms: u64) -> std::path::PathBuf {
    let p = dir.join("claude.json");
    std::fs::write(
        &p,
        json!({"mcpOAuth": {"dumpstar|h": {"serverName": "dumpstar", "serverUrl": format!("{}/mcp", f.base),
               "accessToken": "tok-fresh", "refreshToken": "claude-refresh", "expiresAt": expires_ms}}})
        .to_string(),
    )
    .unwrap();
    p
}
