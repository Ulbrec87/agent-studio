//! Terminal output as plain text for the MCP `studio_get_output` tool.

use std::sync::LazyLock;

use regex::Regex;

static ESCAPES: LazyLock<Regex> = LazyLock::new(|| {
    // OSC (title, hyperlinks), CSI, charset selection and two-character escapes.
    Regex::new(
        r"\x1B\][^\x07\x1B]*(?:\x07|\x1B\\)|\x1B\[[0-?]*[ -/]*[@-~]|\x1B[()][A-Za-z0-9]|\x1B[@-Z\\-_]",
    )
    .expect("static regex")
});

/// Lines of `raw` without escape sequences and control characters. A carriage return rewinds the
/// line (progress bars and redraws keep their last state), trailing blanks go, runs of blank lines
/// collapse to one and the text starts and ends at the first/last non-blank line.
pub fn plain_lines(raw: &str) -> Vec<String> {
    let stripped = ESCAPES.replace_all(raw, "");
    let mut lines: Vec<String> = Vec::new();
    for line in stripped.split('\n') {
        let line = line.trim_end_matches('\r');
        let last = line.rsplit('\r').find(|s| !s.is_empty()).unwrap_or("");
        let clean: String = last
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .collect();
        let clean = clean.trim_end().to_string();
        if clean.is_empty() && lines.last().is_none_or(String::is_empty) {
            continue;
        }
        lines.push(clean);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_escapes_and_collapses() {
        let raw = "\x1b[1;32mhello\x1b[0m\r\n\x1b]0;title\x07\n\n\nworld\x1b[2K\n";
        assert_eq!(plain_lines(raw), ["hello", "", "world"]);
    }

    #[test]
    fn carriage_return_keeps_the_last_redraw() {
        assert_eq!(plain_lines("10%\r50%\r100%\ndone"), ["100%", "done"]);
    }

    #[test]
    fn empty_input() {
        assert!(plain_lines("\x1b[?25l\n\n").is_empty());
    }
}
