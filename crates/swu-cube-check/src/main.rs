//! `swu-cube-check mcp` serves `check_cube` over MCP stdio (see `mcp.rs`).
//! `swu-cube-check login [--status]` signs in once; `pack --deck ID`, `cube|copies --group NAME` and
//! `allocate --group NAME | --plan FILE` then read the site's REST surface themselves.
//! `swu-cube-check allocate [FILE]` plans copy allocation across the whole cube.
//! `swu-cube-check pack|cube|copies [--planned] [FILE...]` (several FILEs only for `cube` and `copies`, one pack each) — reads the document from FILE or stdin, prints one line
//! per failure, and exits 1 when any rule fails (2 when the input cannot be read).

use std::io::Read;
use std::process::ExitCode;

/// Removes `--name VALUE` from `args` and returns VALUE.
fn take_value(args: &mut Vec<String>, name: &str) -> Result<Option<String>, String> {
    let Some(i) = args.iter().position(|a| a == name) else {
        return Ok(None);
    };
    if i + 1 >= args.len() {
        return Err(format!("{name} needs a value"));
    }
    let v = args.remove(i + 1);
    args.remove(i);
    Ok(Some(v))
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let planned = args.iter().any(|a| a == "--planned");
    args.retain(|a| a != "--planned");
    let verbose = args.iter().any(|a| a == "--verbose");
    args.retain(|a| a != "--verbose");
    let fill = args.iter().any(|a| a == "--fill");
    args.retain(|a| a != "--fill");
    let status = args.iter().any(|a| a == "--status");
    args.retain(|a| a != "--status");
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let mut take = |n: &str| take_value(&mut args, n);
    let flags = (|| {
        Ok::<_, String>(Remote {
            site: take("--site")?,
            client_id: take("--client-id")?,
            deck: take("--deck")?,
            group: take("--group")?,
            plan: take("--plan")?,
        })
    })();
    let remote = match flags {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    if args.first().map(String::as_str) == Some("mcp") {
        return mcp(&remote);
    }
    if args.first().map(String::as_str) == Some("login") {
        return login(&remote, status);
    }
    if remote.any() {
        return fetched(&args, &remote, planned, verbose, fill);
    }
    let (mode, paths) = match args.as_slice() {
        [m] => (m.as_str(), vec!["-"]),
        [m, p] => (m.as_str(), vec![p.as_str()]),
        [m, ps @ ..] if m == "cube" || m == "copies" => {
            (m.as_str(), ps.iter().map(String::as_str).collect())
        }
        _ => return usage(),
    };
    if mode == "allocate" {
        return allocate(paths[0], verbose, fill);
    }
    let mut docs = Vec::new();
    for path in &paths {
        let mut text = String::new();
        let read = if *path == "-" {
            std::io::stdin().read_to_string(&mut text).map(|_| ())
        } else {
            std::fs::read_to_string(path).map(|t| text = t)
        };
        if let Err(e) = read {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::from(2);
        }
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => {
                let each = match &v {
                    serde_json::Value::Array(a) => a.iter().collect(),
                    one => vec![one],
                };
                for d in each {
                    if let Err(e) = swu_cube_check::verify_digest(d) {
                        eprintln!("{path}: {e}");
                        return ExitCode::from(2);
                    }
                }
                docs.push(v)
            }
            Err(e) => {
                eprintln!("not JSON: {path}: {e}");
                return ExitCode::from(2);
            }
        }
    }
    // Several files to `cube` are pack documents (or arrays of them), joined into one cube.
    let mut value = if paths.len() > 1
        || (mode == "copies" && docs[0].get("packs").is_none() && !docs[0].is_array())
    {
        let mut packs = Vec::new();
        for d in docs {
            match d {
                serde_json::Value::Array(a) => packs.extend(a),
                other => packs.push(other),
            }
        }
        serde_json::Value::Array(packs)
    } else {
        docs.remove(0)
    };
    if planned {
        swu_cube_check::strip_reservations(&mut value);
    }
    if mode == "copies"
        && let Err(e) = fill_owned(&mut value, remote.site.as_deref())
    {
        return fail(e);
    }
    let result = match mode {
        "pack" => swu_cube_check::check_pack_value(&value),
        "cube" => swu_cube_check::check_cube_value(&value),
        "copies" => swu_cube_check::check_copies_value(&value),
        _ => return usage(),
    };
    report(result)
}

/// A copies check needs `owned_count` on every card; a document without them gets the site's
/// counts, and with no sign-in the check is refused rather than reported `ok`.
fn fill_owned(value: &mut serde_json::Value, site_override: Option<&str>) -> Result<(), String> {
    use swu_cube_check::site;
    // Sign in only when something is missing.
    let probe = swu_cube_check::fetch::fill_owned(&NoSite, &mut value.clone());
    if probe.is_ok() {
        return Ok(());
    }
    let src = site::Site::open(&site::config_path(), site_override).map_err(|e| {
        format!("cannot check copies: the document carries no owned_count and the site's counts need a sign-in ({e})")
    });
    swu_cube_check::fetch::fill_owned(&src?, value).map(|_| ())
}

/// A source that holds nothing: filling succeeds against it only when no card is missing a count.
struct NoSite;

impl swu_cube_check::fetch::Source for NoSite {
    fn list_group(&self, _: &str) -> Result<Vec<i64>, String> {
        Err("no site".into())
    }
    fn deck(&self, _: i64) -> Result<serde_json::Value, String> {
        Err("no site".into())
    }
    fn search(&self, _: &str) -> Result<Vec<serde_json::Value>, String> {
        Err("no site".into())
    }
}

fn allocate(path: &str, verbose: bool, fill: bool) -> ExitCode {
    let mut text = String::new();
    let read = if path == "-" {
        std::io::stdin().read_to_string(&mut text).map(|_| ())
    } else {
        std::fs::read_to_string(path).map(|t| text = t)
    };
    if let Err(e) = read {
        eprintln!("cannot read {path}: {e}");
        return ExitCode::from(2);
    }
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) => allocate_value(&v, verbose, fill),
        Err(e) => fail(format!("not JSON: {path}: {e}")),
    }
}

fn allocate_value(doc: &serde_json::Value, verbose: bool, fill: bool) -> ExitCode {
    match swu_cube_check::allocate::allocate_value(doc, fill) {
        Err(e) => fail(e),
        Ok(plan) => {
            let (code, text) = swu_cube_check::allocate::render(&plan, verbose);
            println!("{text}");
            ExitCode::from(code as u8)
        }
    }
}

const USAGE: &str = "usage: swu-cube-check mcp   (serve check_cube over MCP stdio; the plugin's .mcp.json runs this)\n       swu-cube-check login [--site URL] [--client-id ID] | login --status
       swu-cube-check pack --deck ID | cube --group NAME | copies --group NAME | allocate --group NAME | allocate --plan FILE   [--site URL]
  credential order: Claude Code's own MCP sign-in for the site (read-only, from ~/.claude/.credentials.json or the macOS
            Keychain; never written, its refresh token never used), then the login below
  login     sign in once in a browser (authorization code + PKCE); the refresh token is cached at
            ~/.config/dumpstar/swu-cube-check.json (0600) and never printed; --status says whose tenant it is
  --deck/--group/--plan  fetch decks and run searches over the site's /api yourself; every deck's lines_digest is checked.
            An expired or rejected sign-in exits 2 naming `login`; there is no fallback. allocate --plan FILE is
            {\"packs\": [{\"name\", \"queries\": [search strings], \"deck\": ID?, \"picks\": [...]}]} with the rows fetched for you
       swu-cube-check pack [--planned] [FILE] | cube [--planned] [FILE...] | copies [--planned] [FILE...] | allocate [--verbose] [--fill] [FILE]
  pack      check one pack document (get_deck's result) against the pack rules
  cube      check a 20-pack cube: a cube document, or one pack file per deck
  copies    only the copy-count check (cube-copies) over any number of pack files, for a partial build
  allocate  plan copies across the cube (one line per pack: picks of slots (+ filled with --fill), eligible pool, skeleton ok/unmet/not checked, contested reserved, short, lost; --verbose adds every
            reserve line; a Leader or non-base Rare/Legendary goes to at most one pack): {\"owned\": {id: copies}, \"packs\": [{\"name\", \"slots\": 15,
            \"eligible\": [search_cards rows or card ids], \"picks\": [...], \"candidates\": [...]}]}; only picks and candidates reserve; rows carry type, cost and arenas, so a
            15-slot pack must be able to meet the pack skeleton from its pool, or gets a named shortfall; the plan invents no picks: --fill chooses 15 skeleton cards for each
            such pack and counts them as filled, reserved and contested like picks; `contested reserved` counts only copies more packs want than are owned, so 0 is normal
  FILE is read in place of stdin (`-` is stdin); exit 0 ok, 1 a rule failed, 2 unreadable input";

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

struct Remote {
    site: Option<String>,
    client_id: Option<String>,
    deck: Option<String>,
    group: Option<String>,
    plan: Option<String>,
}

impl Remote {
    fn any(&self) -> bool {
        self.deck.is_some() || self.group.is_some() || self.plan.is_some()
    }
}

fn fail(m: impl std::fmt::Display) -> ExitCode {
    eprintln!("{m}");
    ExitCode::from(2)
}

fn login(r: &Remote, status: bool) -> ExitCode {
    use swu_cube_check::site;
    let path = site::config_path();
    if status {
        let shown = site::credential(&path, r.site.as_deref()).and_then(|c| {
            let label = c.label.clone();
            site::Site::with(c)?.tenant().map(|t| (label, t))
        });
        return match shown {
            Ok((label, t)) => {
                println!("{label} \u{2192} tenant {t}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        };
    }
    let old = site::load(&path).ok().flatten();
    let site_url = r
        .site
        .clone()
        .or_else(|| old.as_ref().map(|c| c.site.clone()))
        .or_else(|| std::env::var("DUMPSTAR_SITE").ok())
        .unwrap_or_else(|| site::DEFAULT_SITE.to_string());
    let Some(client_id) = r
        .client_id
        .clone()
        .or_else(|| old.map(|c| c.client_id))
        .or_else(|| std::env::var("SWU_CUBE_CHECK_CLIENT_ID").ok())
    else {
        return fail(
            "login needs the issuer's client id: --client-id ID (or SWU_CUBE_CHECK_CLIENT_ID); it is cached afterwards",
        );
    };
    let announce = |url: &str| eprintln!("Open this address in a browser and sign in:\n{url}");
    match site::login(&path, &site_url, &client_id, &announce) {
        Ok(c) => {
            println!("signed in to {} as {}", c.site, c.who);
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

/// `swu-cube-check mcp`: the `check_cube` tool over MCP stdio, until the client closes stdin.
fn mcp(r: &Remote) -> ExitCode {
    use swu_cube_check::{mcp, site};
    let default_site = r.site.clone();
    let open =
        move |site_arg: Option<&str>| -> Result<Box<dyn swu_cube_check::fetch::Source>, String> {
            let over = site_arg.or(default_site.as_deref());
            Ok(Box::new(site::Site::open(&site::config_path(), over)?))
        };
    match mcp::serve(std::io::stdin().lock(), std::io::stdout().lock(), &open) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(format!("mcp: {e}")),
    }
}

fn fetched(args: &[String], r: &Remote, planned: bool, verbose: bool, fill: bool) -> ExitCode {
    use swu_cube_check::{fetch, site};
    let [mode] = args else { return usage() };
    let src = match site::Site::open(&site::config_path(), r.site.as_deref()) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let load = || -> Result<serde_json::Value, String> {
        match (mode.as_str(), &r.deck, &r.group, &r.plan) {
            ("pack", Some(id), None, None) => {
                let id: i64 = id
                    .parse()
                    .map_err(|_| format!("--deck wants a deck id, not '{id}'"))?;
                swu_cube_check::fetch::Source::deck(&src, id)
            }
            ("cube" | "copies", None, Some(g), None) => {
                Ok(serde_json::Value::Array(fetch::group_decks(&src, g)?))
            }
            ("allocate", None, g, p) if g.is_some() || p.is_some() => {
                let plan = match p {
                    Some(p) => Some(
                        std::fs::read_to_string(p)
                            .map_err(|e| format!("cannot read {p}: {e}"))
                            .and_then(|t| {
                                serde_json::from_str(&t).map_err(|e| format!("not JSON: {p}: {e}"))
                            })?,
                    ),
                    None => None,
                };
                fetch::allocation(&src, g.as_deref(), plan.as_ref())
            }
            _ => Err(USAGE.to_string()),
        }
    };
    let mut value = match load() {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    if mode == "allocate" {
        return allocate_value(&value, verbose, fill);
    }
    if planned {
        swu_cube_check::strip_reservations(&mut value);
    }
    report(match mode.as_str() {
        "pack" => swu_cube_check::check_pack_value(&value),
        "cube" => swu_cube_check::check_cube_value(&value),
        _ => swu_cube_check::check_copies_value(&value),
    })
}

fn report(result: Result<Vec<swu_cube_check::Failure>, String>) -> ExitCode {
    let (code, text) = swu_cube_check::render(result);
    if code == 2 {
        eprintln!("{text}");
    } else {
        println!("{text}");
    }
    ExitCode::from(code as u8)
}
