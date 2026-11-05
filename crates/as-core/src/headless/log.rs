//! Reading the tail of a run's JSONL log (Hydra `HeadlessOrchestrator.getLog`).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::types::{
    DEFAULT_LOG_MAX_CHARS, DEFAULT_LOG_TAIL_LINES, HeadlessRunLog, HeadlessRunLogOptions,
};

/// The last `tail_lines` non-empty lines of `path`, at most `max_chars` characters of them
/// (the oldest characters are cut, so the first line may be partial; `truncated` says so).
/// Streams the file, so a huge log costs no memory beyond the tail.
pub fn read_log_tail(path: &Path, run_id: &str, options: &HeadlessRunLogOptions) -> HeadlessRunLog {
    let tail_lines = options
        .tail_lines
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_LOG_TAIL_LINES);
    let max_chars = options
        .max_chars
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_LOG_MAX_CHARS);
    let empty = HeadlessRunLog {
        run_id: run_id.to_string(),
        content: String::new(),
        total_lines: 0,
        returned_lines: 0,
        truncated: false,
    };
    let Ok(file) = std::fs::File::open(path) else {
        return empty;
    };
    let mut reader = BufReader::new(file);
    let mut tail: VecDeque<String> = VecDeque::with_capacity(tail_lines.min(4096));
    let mut total = 0usize;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim_end_matches(['\n', '\r']);
        if line.is_empty() {
            continue;
        }
        total += 1;
        if tail.len() == tail_lines {
            tail.pop_front();
        }
        tail.push_back(line.to_string());
    }
    let mut truncated = total > tail.len();
    let mut content = tail
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    let chars = content.chars().count();
    if chars > max_chars {
        content = content.chars().skip(chars - max_chars).collect();
        truncated = true;
    }
    let returned_lines = if content.is_empty() {
        0
    } else {
        content.split('\n').count()
    };
    HeadlessRunLog {
        run_id: run_id.to_string(),
        content,
        total_lines: total,
        returned_lines,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_of(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn tail_lines_and_counts() {
        let (_d, path) = log_of("line1\nline2\nline3\nline4\nline5");
        let log = read_log_tail(
            &path,
            "r",
            &HeadlessRunLogOptions {
                tail_lines: Some(2),
                max_chars: Some(1000),
            },
        );
        assert_eq!(log.content, "line4\nline5");
        assert_eq!((log.total_lines, log.returned_lines), (5, 2));
        assert!(log.truncated);
    }

    #[test]
    fn whole_log_is_not_truncated() {
        let (_d, path) = log_of("a\n\nb\n");
        let log = read_log_tail(&path, "r", &HeadlessRunLogOptions::default());
        assert_eq!(log.content, "a\nb");
        assert!(!log.truncated);
        assert_eq!(log.total_lines, 2);
    }

    #[test]
    fn char_cap_keeps_the_end_and_reports_truncation() {
        let (_d, path) = log_of("0123456789\nabcdefghij\n");
        let log = read_log_tail(
            &path,
            "r",
            &HeadlessRunLogOptions {
                tail_lines: None,
                max_chars: Some(13),
            },
        );
        assert_eq!(log.content, "89\nabcdefghij");
        assert!(log.truncated);
        assert_eq!(log.returned_lines, 2);
    }

    #[test]
    fn missing_file_is_empty() {
        let log = read_log_tail(
            std::path::Path::new("/definitely/not/here.jsonl"),
            "r",
            &HeadlessRunLogOptions::default(),
        );
        assert_eq!(log.total_lines, 0);
        assert!(!log.truncated);
    }
}
