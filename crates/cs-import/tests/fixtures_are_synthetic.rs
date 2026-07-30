//! The guard that keeps `tests/fixtures/` hand-authored (bead chat-search-4ar.3).
//!
//! The fixtures are the reason this repo can be shared: real transcripts carry paths,
//! hostnames, tokens and machine identity, and a golden cut from one cannot be reviewed in
//! public. Authoring them from scratch makes that claim structural — but only on the day
//! they are written. The failure this file exists to prevent is the slow one: somebody
//! debugging chat-search-n58.x drops a captured rollout in beside the synthetic ones, or
//! regenerates a golden from a real corpus, and the tree quietly stops being publishable.
//!
//! So the properties are checked, not asserted once in a commit message. They are deliberately
//! stricter than "no secrets": a fixture may not contain a UUID *at all*, may not name a home
//! directory other than the invented `/home/dev/…`, and may not carry anything shaped like a
//! credential. Real transcripts fail every one of those on the first line — session ids are
//! UUIDs, `cwd` is a real home — so pasting one in fails here rather than in review.
//!
//! Only `tests/fixtures/` is scanned. The patterns themselves live in this file, one
//! directory up, so the guard cannot flag its own source.
//!
//! If a future fixture genuinely needs a UUID-shaped id, give it the reserved synthetic form
//! `00000000-0000-4000-8000-…` and widen `is_synthetic_uuid` deliberately. Widening it is a
//! decision; a real id sliding in is an accident.

use std::path::{Path, PathBuf};

/// The one home directory the fixtures are allowed to name. Invented, and not a real login
/// on any machine this project runs on.
const FIXTURE_HOME: &str = "/home/dev/";

/// Prefixes that mean "this is a live credential". Prefix matching rather than entropy
/// scoring: these are the shapes that actually ride along in a transcript — an env dump, a
/// `curl` line, a pasted 401 — and a fixed prefix cannot fire on prose the way an entropy
/// threshold does, so nobody is ever tempted to relax it.
const CREDENTIAL_SHAPES: [&str; 14] = [
    "sk-",           // OpenAI, Anthropic
    "sk_live_",      // Stripe, live
    "sk_test_",      // Stripe, test — still a secret, and still not ours to publish
    "ghp_",          // GitHub personal access token
    "gho_",          // GitHub OAuth
    "ghs_",          // GitHub app server-to-server
    "github_pat_",   // GitHub fine-grained
    "glpat-",        // GitLab
    "xoxb-",         // Slack bot
    "xoxp-",         // Slack user
    "AKIA",          // AWS long-lived access key id
    "ASIA",          // AWS session key id
    "AIza",          // Google API key
    "-----BEGIN",    // any PEM private key
];

#[test]
fn no_fixture_names_a_real_home_directory() {
    scan("a home directory this project did not invent", names_a_foreign_home);
}

#[test]
fn no_fixture_contains_a_uuid() {
    // The machine identity is a v4 UUID (ADR 17) and so is every Claude Code `sessionId`,
    // every ChatGPT conversation id and every Codex rollout id. Banning the *shape* checks
    // all of them at once, and without reading a single real file to compare against —
    // which is the point: a guard that needs the secret in order to look for it is a guard
    // that only works on the machine holding the secret.
    scan("a UUID, which a hand-authored id never needs", has_foreign_uuid);
}

#[test]
fn no_fixture_contains_a_credential_shape() {
    scan("something shaped like a credential", has_credential_shape);
}

#[test]
fn no_fixture_contains_a_hostname_or_an_email_address() {
    // `.local` is what a Mac calls itself on the LAN, and it is how a hostname reaches a
    // transcript; an address is the other identifier that travels in prose.
    scan("a hostname or an email address", has_hostname_or_email);
}

#[test]
fn no_fixture_contains_this_machines_identity() {
    // The checks above are structural and hold on any machine, including CI. This one is
    // the opposite: it knows who is running it, and catches the paste that happens to use a
    // shape the other tests allow. Generic account names are skipped — a CI box whose user
    // is literally `dev` would otherwise fail on the invented `/home/dev`, which is a false
    // alarm rather than a leak.
    const GENERIC: [&str; 8] = ["dev", "user", "users", "root", "admin", "test", "ci", "runner"];

    let mut needles: Vec<String> = Vec::new();
    for var in ["HOME", "USER", "LOGNAME", "USERNAME", "HOSTNAME"] {
        let Ok(value) = std::env::var(var) else { continue };
        let value = value.trim().to_string();
        if value.len() >= 4 && !GENERIC.contains(&value.to_ascii_lowercase().as_str()) {
            needles.push(value);
        }
    }

    let mut bad = Vec::new();
    for (path, text) in fixture_files() {
        for (n, line) in text.lines().enumerate() {
            for needle in &needles {
                if line.contains(needle.as_str()) {
                    bad.push(format!("{} names `{needle}`", hit(&path, n, line)));
                }
            }
        }
    }
    assert!(bad.is_empty(), "fixtures name the machine they were authored on:\n{}", bad.join("\n"));
}

#[test]
fn the_fixture_tree_is_readable_text_and_not_empty() {
    // `fixture_files` reads every file as UTF-8, so a binary blob — an archived `.zst`, a
    // screenshot — fails here rather than being skipped silently by the scans above.
    let files = fixture_files();
    assert!(files.len() >= 4, "expected the four documented shapes, found {}", files.len());
    let all_have_content = files.iter().all(|(_, text)| !text.trim().is_empty());
    assert!(all_have_content, "an empty fixture proves nothing");
}

#[test]
fn the_guard_fires_on_what_it_claims_to_catch() {
    // A guard that has only ever been run against clean input is a guard nobody has tested.
    // These are the lines that would actually appear if a real transcript were dropped into
    // the tree — the ones above prove the fixtures are clean, and this one proves that
    // means something.
    let leaks = [
        r#"{"cwd":"/Users/rwilson/dev/example-project"}"#,
        r#"{"cwd":"/home/rwilson/example-project"}"#,
        r#"{"cwd":"C:\\Users\\rwilson\\example-project"}"#,
        r#"{"session_id":"019ec267-4f1a-7b2c-9d3e-5a6b7c8d9e0f"}"#,
        r#"{"machine":"3f2504e0-4f89-41d3-9a0c-0305e82c3301"}"#,
        &format!(r#"{{"env":"OPENAI_API_KEY={}"}}"#, "sk-".to_owned() + "not-a-real-key"),
        r#"{"header":"Authorization: Bearer ghp_notarealtokeneither"}"#,
        r#"{"host":"someones-macbook-air.local"}"#,
        r#"{"text":"mail me at someone@example.com"}"#,
    ];
    for line in leaks {
        assert!(
            names_a_foreign_home(line)
                || has_foreign_uuid(line)
                || has_credential_shape(line)
                || has_hostname_or_email(line),
            "the guard would have let this through: {line}"
        );
    }

    // And the fixtures' own vocabulary must stay legal, or the guard gets weakened the first
    // time it cries wolf.
    for clean in [
        r#"{"cwd":"/home/dev/example-project"}"#,
        r#"{"id":"conv-a1b2","session_id":"conv-a1b2"}"#,
        r#"{"text":"src/cache/widget.rs:41: fn clear(&mut self) {"}"#,
        r#"{"customTitle":"Widget cache rename bug"}"#,
        r#"{"id":"00000000-0000-4000-8000-000000000001"}"#,
    ] {
        assert!(
            !(names_a_foreign_home(clean)
                || has_foreign_uuid(clean)
                || has_credential_shape(clean)
                || has_hostname_or_email(clean)),
            "the guard fires on legitimate fixture content: {clean}"
        );
    }
}

// ----------------------------------------------------------------- predicates

/// `/Users/` is the macOS shape this corpus was captured on; the Windows and Linux
/// spellings cost a line each and cover a machine that has not run this yet.
fn names_a_foreign_home(line: &str) -> bool {
    line.contains("/Users/")
        || line.contains("\\Users\\")
        || line.contains("/root/")
        || line.match_indices("/home/").any(|(at, _)| !line[at..].starts_with(FIXTURE_HOME))
}

fn has_foreign_uuid(line: &str) -> bool {
    uuids_in(line).any(|u| !is_synthetic_uuid(&u))
}

fn has_credential_shape(line: &str) -> bool {
    CREDENTIAL_SHAPES.iter().any(|shape| line.contains(shape))
}

fn has_hostname_or_email(line: &str) -> bool {
    line.contains(".local") || has_email(line)
}


// ------------------------------------------------------------------ scanning

/// Every line of every fixture that trips `predicate`, reported together and located by file
/// and line: a guard that says only "something leaked" sends the reader back to grep.
fn scan(what: &str, predicate: fn(&str) -> bool) {
    let mut bad = Vec::new();
    for (path, text) in fixture_files() {
        for (n, line) in text.lines().enumerate() {
            if predicate(line) {
                bad.push(hit(&path, n, line));
            }
        }
    }
    assert!(bad.is_empty(), "fixtures contain {what}:\n{}", bad.join("\n"));
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

/// Every file under the fixture tree — inputs *and* goldens. The goldens matter as much:
/// they are derived from the inputs today, but a regeneration pointed at a real corpus
/// would land here first.
fn fixture_files() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    walk(&fixtures_root(), &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
            continue;
        }
        let bytes =
            std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let text = String::from_utf8(bytes).unwrap_or_else(|_| {
            panic!("{} is not UTF-8; fixtures are hand-written text", path.display())
        });
        out.push((path, text));
    }
}

fn hit(path: &Path, line_index: usize, line: &str) -> String {
    let name = path.strip_prefix(fixtures_root()).unwrap_or(path).display();
    let snippet: String = line.trim().chars().take(120).collect();
    format!("  {name}:{}: {snippet}", line_index + 1)
}

/// Canonical `8-4-4-4-12` hex runs. Hand-rolled rather than pulling in a regex crate for one
/// pattern, in the same spirit as the importers' hand-rolled RFC 3339 parsing.
fn uuids_in(line: &str) -> impl Iterator<Item = String> + '_ {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let bytes = line.as_bytes();
    (0..bytes.len()).filter_map(move |start| {
        // A run that continues into more hex on either side is a longer token, not an id.
        if start > 0 && is_uuid_byte(bytes[start - 1]) {
            return None;
        }
        let mut at = start;
        for (i, len) in GROUPS.iter().enumerate() {
            if i > 0 {
                if bytes.get(at) != Some(&b'-') {
                    return None;
                }
                at += 1;
            }
            let group = bytes.get(at..at + len)?;
            if !group.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            at += len;
        }
        (!bytes.get(at).is_some_and(|b| is_uuid_byte(*b))).then(|| line[start..at].to_string())
    })
}

fn is_uuid_byte(b: u8) -> bool {
    b.is_ascii_hexdigit() || b == b'-'
}

/// The reserved shape a fixture may use when it needs a UUID-typed id: all-zero except for
/// the version and variant nibbles and a short counting tail.
fn is_synthetic_uuid(candidate: &str) -> bool {
    candidate.to_ascii_lowercase().starts_with("00000000-0000-4000-8000-")
}

/// A conservative `local@domain.tld`: alphanumeric on the left, a dotted domain of at least
/// two labels on the right. Good enough to catch an address pasted into prose, and it does
/// not fire on `@mention` or on a `user@` fragment.
fn has_email(line: &str) -> bool {
    line.match_indices('@').any(|(at, _)| {
        let left = line[..at].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric());
        let domain: String = line[at + 1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
            .collect();
        let dotted = domain.rsplit_once('.').is_some_and(|(host, tld)| {
            !host.is_empty() && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
        });
        left && dotted
    })
}
