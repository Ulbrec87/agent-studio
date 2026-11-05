//! Gate for the automatic startup input of Claude agents (`/rename <name>`, the initial prompt).
//!
//! Claude asks "Do you trust the files in this folder?" in a folder it has not seen yet, with
//! **"No, exit" pre-selected**. Hydra writes `/rename <name>\r` 1.5 s after spawn, which presses
//! Enter on "No, exit": the agent exits and ends `errored`. [`TrustGate`] watches the agent's output
//! and holds those writes while the prompt is on screen:
//!
//! 1. the prompt is detected in the (ANSI-stripped) output ([`contains_trust_prompt`]);
//! 2. the user answers it (they press Enter or a digit; arrow keys only move the selection);
//! 3. new output arrives that no longer shows the prompt (the dialog is gone and the real UI is
//!    drawn); if the prompt shows up again (a redraw after an arrow key) the wait restarts;
//! 4. [`StartupOptions::trust_settle`] later the held writes go out.
//!
//! The same gate covers Claude's other first-run dialogs that Enter would answer for the user: the
//! project MCP server approval ("New MCP server found in .mcp.json", default "Use this and all
//! future MCP servers") and the custom API key prompt ("Detected a custom API key in your
//! environment. Do you want to use this API key?"). They are matched by [`DIALOG_PHRASES`] and follow
//! the same hold / answer / settle sequence (answer = Enter or a digit).
//!
//! If that does not happen within [`StartupOptions::trust_timeout`] the writes are dropped (and
//! logged); when the process exits they are dropped silently. Without a prompt the writes go out
//! on Hydra's schedule, once the CLI printed anything (or after [`StartupOptions::no_output_grace`]).

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use super::model_detect::strip_ansi;

/// Phrases of the first-run dialogs, compared after removing everything but letters and digits
/// (the terminal UI may position words with cursor moves instead of spaces).
const DIALOG_PHRASES: [&str; 12] = [
    // Folder trust.
    "trustthisfolder",
    "trustthefilesinthisfolder",
    "doyoutrust",
    "isthisaprojectyou",
    "yesitrust",
    // Project MCP server approval (`.mcp.json`).
    "newmcpserverfound",
    "newmcpserversfound",
    "usethisandallfuturemcpservers",
    "continuewithoutusingthismcpserver",
    "continuewithoutusingthesemcpservers",
    // Custom API key prompt.
    "detectedacustomapikey",
    "doyouwanttousethisapikey",
];
/// Output kept to look for the prompt in (bytes).
const TAIL_BYTES: usize = 8192;
/// Visible characters the output after the answer needs before the dialog counts as gone.
const MIN_VISIBLE_AFTER_ANSWER: usize = 40;

static NON_ALNUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").expect("static regex"));

/// Timing knobs (tests shrink them).
#[derive(Debug, Clone, Copy)]
pub struct StartupOptions {
    /// Wait after the dialog disappeared before writing.
    pub trust_settle: Duration,
    /// Give up (drop the startup input) when the dialog is still unanswered after this long.
    pub trust_timeout: Duration,
    /// How often the held writes re-check the gate.
    pub poll: Duration,
    /// Without any output from the CLI, write anyway after this long.
    pub no_output_grace: Duration,
}

impl Default for StartupOptions {
    fn default() -> Self {
        Self {
            trust_settle: Duration::from_millis(1500),
            trust_timeout: Duration::from_secs(300),
            poll: Duration::from_millis(100),
            no_output_grace: Duration::from_secs(10),
        }
    }
}

/// Is one of Claude's first-run dialogs ("do you trust this folder", MCP server approval, custom
/// API key) in `raw` (terminal output, escapes allowed)?
pub fn contains_trust_prompt(raw: &str) -> bool {
    let plain = strip_ansi(raw).to_lowercase();
    let squashed = NON_ALNUM.replace_all(&plain, "");
    DIALOG_PHRASES.iter().any(|p| squashed.contains(p))
}

fn visible_chars(raw: &str) -> usize {
    strip_ansi(raw)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .count()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    /// Write now.
    Proceed,
    /// Keep holding.
    Wait,
    /// Held too long: drop the startup input.
    Expired,
}

/// Per-process state of the startup gate; time is epoch/monotonic milliseconds from the caller.
#[derive(Debug)]
pub struct TrustGate {
    started_ms: u64,
    /// Output since the last user input (or the whole run before the first one), raw bytes.
    tail: Vec<u8>,
    saw_output: bool,
    prompt_seen: bool,
    answered: bool,
    cleared_at: Option<u64>,
}

impl TrustGate {
    pub fn new(now_ms: u64) -> Self {
        Self {
            started_ms: now_ms,
            tail: Vec::new(),
            saw_output: false,
            prompt_seen: false,
            answered: false,
            cleared_at: None,
        }
    }

    /// The trust dialog was seen and the user has not got past it yet.
    pub fn holding(&self) -> bool {
        self.prompt_seen && self.cleared_at.is_none()
    }

    pub fn on_output(&mut self, data: &[u8], now_ms: u64) {
        if data.is_empty() {
            return;
        }
        self.saw_output = true;
        self.tail.extend_from_slice(data);
        if self.tail.len() > TAIL_BYTES {
            let cut = self.tail.len() - TAIL_BYTES;
            self.tail.drain(..cut);
        }
        let text = String::from_utf8_lossy(&self.tail);
        if contains_trust_prompt(&text) {
            // (Re)drawn: still on screen. Forget the text so a later chunk is judged on its own.
            self.prompt_seen = true;
            self.cleared_at = None;
            self.tail.clear();
        } else if self.prompt_seen
            && self.answered
            && self.cleared_at.is_none()
            && visible_chars(&text) >= MIN_VISIBLE_AFTER_ANSWER
        {
            self.cleared_at = Some(now_ms);
        }
    }

    /// The user sent `data` to the agent (keys, paste or a submitted message). Enter or a digit
    /// answers the dialog; arrow keys and other keys do not.
    pub fn on_user_input(&mut self, data: &[u8]) {
        let answers = data
            .iter()
            .any(|b| matches!(b, b'\r' | b'\n' | b'1'..=b'9'));
        if answers && self.prompt_seen && self.cleared_at.is_none() {
            self.answered = true;
            // Only what the agent prints from now on tells whether the dialog is gone.
            self.tail.clear();
        }
    }

    pub fn poll(&self, now_ms: u64, options: &StartupOptions) -> GateState {
        let waited = Duration::from_millis(now_ms.saturating_sub(self.started_ms));
        if !self.prompt_seen {
            return if self.saw_output || waited >= options.no_output_grace {
                GateState::Proceed
            } else {
                GateState::Wait
            };
        }
        if let Some(at) = self.cleared_at
            && Duration::from_millis(now_ms.saturating_sub(at)) >= options.trust_settle
        {
            return GateState::Proceed;
        }
        if waited >= options.trust_timeout {
            GateState::Expired
        } else {
            GateState::Wait
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROMPT: &str = "\x1b[1m Do you trust the files in this folder?\x1b[0m\r\n \x1b[36m\u{276f} 1. Yes, I trust this folder\x1b[0m\r\n   2. No, exit\r\n";
    const MAIN_UI: &str = "\x1b[2J\x1b[H\u{256d}\u{2500} Welcome to Claude Code \u{2500}\u{256e}\r\n> Try \"how does this project work?\"\r\n? for shortcuts";

    fn opts() -> StartupOptions {
        StartupOptions::default()
    }

    #[test]
    fn recognises_the_dialog_in_its_wordings_and_escape_styles() {
        assert!(contains_trust_prompt(PROMPT));
        assert!(contains_trust_prompt(
            "Is this a project you created or one you trust?"
        ));
        assert!(contains_trust_prompt(
            "Quick safety check: Is this a project you trust?"
        ));
        assert!(contains_trust_prompt("DO YOU TRUST this workspace"));
        // Words placed with cursor moves instead of spaces.
        assert!(contains_trust_prompt(
            "Yes,\x1b[1CI\x1b[1Ctrust\x1b[1Cthis\x1b[1Cfolder"
        ));
        assert!(!contains_trust_prompt(MAIN_UI));
        assert!(!contains_trust_prompt(
            "I trust my tests; session management"
        ));
    }

    #[test]
    fn without_a_prompt_it_proceeds_once_the_cli_printed_something() {
        let mut g = TrustGate::new(0);
        assert_eq!(
            g.poll(1500, &opts()),
            GateState::Wait,
            "nothing printed yet"
        );
        g.on_output(MAIN_UI.as_bytes(), 1000);
        assert_eq!(g.poll(1500, &opts()), GateState::Proceed);
        // A silent CLI is written to after the grace period.
        assert_eq!(TrustGate::new(0).poll(10_000, &opts()), GateState::Proceed);
    }

    #[test]
    fn holds_while_the_prompt_is_unanswered_and_expires() {
        let mut g = TrustGate::new(0);
        g.on_output(PROMPT.as_bytes(), 500);
        assert!(g.holding());
        assert_eq!(g.poll(1500, &opts()), GateState::Wait);
        assert_eq!(g.poll(60_000, &opts()), GateState::Wait);
        assert_eq!(g.poll(300_000, &opts()), GateState::Expired);
    }

    #[test]
    fn output_without_user_input_never_releases_the_hold() {
        let mut g = TrustGate::new(0);
        g.on_output(PROMPT.as_bytes(), 500);
        g.on_output(MAIN_UI.as_bytes(), 800);
        assert!(g.holding());
        assert_eq!(g.poll(20_000, &opts()), GateState::Wait);
    }

    #[test]
    fn released_after_the_answer_new_output_and_the_settle_time() {
        let mut g = TrustGate::new(0);
        g.on_output(PROMPT.as_bytes(), 500);
        g.on_user_input(b"\r");
        assert_eq!(g.poll(2000, &opts()), GateState::Wait, "no new output yet");
        g.on_output(MAIN_UI.as_bytes(), 3000);
        assert!(!g.holding());
        assert_eq!(g.poll(3000, &opts()), GateState::Wait, "settling");
        assert_eq!(g.poll(4499, &opts()), GateState::Wait);
        assert_eq!(g.poll(4500, &opts()), GateState::Proceed);
    }

    #[test]
    fn a_redraw_of_the_prompt_restarts_the_wait() {
        let mut g = TrustGate::new(0);
        g.on_output(PROMPT.as_bytes(), 500);
        g.on_user_input(b"\x1b[B"); // Down only moves the selection
        g.on_output(PROMPT.as_bytes(), 600);
        g.on_output(MAIN_UI.as_bytes(), 650); // (even unrelated output is not an answer)
        assert!(g.holding());
        g.on_user_input(b"\r"); // Enter
        g.on_output(b"\x1b[2K", 700); // a few escapes are not the main UI yet
        assert!(g.holding());
        g.on_output(MAIN_UI.as_bytes(), 800);
        assert!(!g.holding());
        // Late redraw of the dialog during the settle time resets it.
        g.on_output(PROMPT.as_bytes(), 1000);
        assert!(g.holding());
        g.on_output(MAIN_UI.as_bytes(), 1100);
        assert_eq!(g.poll(2599, &opts()), GateState::Wait);
        assert_eq!(g.poll(2600, &opts()), GateState::Proceed);
    }

    #[test]
    fn prompt_split_across_chunks_is_found() {
        let mut g = TrustGate::new(0);
        g.on_output(b"\x1b[1mDo you tr", 10);
        assert!(!g.holding());
        g.on_output(b"ust the files in this folder?", 20);
        assert!(g.holding());
    }

    #[test]
    fn user_input_before_the_prompt_does_not_count_as_an_answer() {
        let mut g = TrustGate::new(0);
        g.on_user_input(b"\r");
        g.on_output(PROMPT.as_bytes(), 500);
        g.on_output(MAIN_UI.as_bytes(), 600);
        assert!(g.holding());
    }

    const MCP_PROMPT: &str = "\x1b[1m New MCP server found in .mcp.json: studio\x1b[0m\r\n\r\n \x1b[36m\u{276f} 1. Use this and all future MCP servers in this project\x1b[0m\r\n   2. Use this MCP server\r\n   3. Continue without using this MCP server\r\n";
    const API_KEY_PROMPT: &str = "Detected a custom API key in your environment\r\nsk-ant-...xyz\r\nDo you want to use this API key?\r\n \u{276f} 1. Yes\r\n   2. No (recommended)\r\n";

    #[test]
    fn recognises_the_mcp_approval_and_api_key_dialogs() {
        assert!(contains_trust_prompt(MCP_PROMPT));
        assert!(contains_trust_prompt(
            "3 new MCP servers found in .mcp.json"
        ));
        assert!(contains_trust_prompt(
            "Continue\x1b[1Cwithout\x1b[1Cusing\x1b[1Cthese\x1b[1CMCP\x1b[1Cservers"
        ));
        assert!(contains_trust_prompt(API_KEY_PROMPT));
        // Talking about MCP servers is not the dialog.
        assert!(!contains_trust_prompt(
            "You can add an MCP server with `claude mcp add`; my API key is set."
        ));
        assert!(!contains_trust_prompt(MAIN_UI));
    }

    #[test]
    fn the_mcp_dialog_holds_the_startup_input_until_answered() {
        let mut g = TrustGate::new(0);
        g.on_output(MCP_PROMPT.as_bytes(), 500);
        assert!(g.holding());
        assert_eq!(g.poll(5_000, &opts()), GateState::Wait);
        // Moving the selection is not an answer.
        g.on_user_input(b"\x1b[B");
        g.on_output(MCP_PROMPT.as_bytes(), 5_100);
        assert!(g.holding());
        g.on_user_input(b"\r");
        g.on_output(MAIN_UI.as_bytes(), 6_000);
        assert!(!g.holding());
        assert_eq!(g.poll(7_499, &opts()), GateState::Wait);
        assert_eq!(g.poll(7_500, &opts()), GateState::Proceed);
    }

    #[test]
    fn the_api_key_dialog_holds_and_expires_like_the_trust_dialog() {
        let mut g = TrustGate::new(0);
        g.on_output(API_KEY_PROMPT.as_bytes(), 100);
        assert!(g.holding());
        assert_eq!(g.poll(299_999, &opts()), GateState::Wait);
        assert_eq!(g.poll(300_000, &opts()), GateState::Expired);
        // A digit answers it.
        g.on_user_input(b"2");
        g.on_output(MAIN_UI.as_bytes(), 400_000);
        assert_eq!(g.poll(401_500, &opts()), GateState::Proceed);
    }
}
