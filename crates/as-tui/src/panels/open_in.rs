//! "Open in …" picker: editors, file manager and terminal that look usable on this machine.

use as_core::open_in::{OpenOption, OpenTarget};
use as_proto::{Request, Response};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{Outcome, Reply, nav, req, step};
use crate::app::Effect;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenInUi {
    pub path: String,
    pub options: Vec<OpenOption>,
    pub cursor: usize,
    pub loaded: bool,
}

impl OpenInUi {
    pub fn open(&mut self, path: &str) -> Vec<Effect> {
        self.path = path.to_string();
        self.cursor = 0;
        vec![req("open:list", Request::ListOpenTargets)]
    }

    pub fn on_reply(&mut self, key: &str, result: Reply) {
        if key == "open:list"
            && let Ok(Response::OpenTargets(options)) = result
        {
            self.options = options;
            self.loaded = true;
            self.cursor = self.cursor.min(self.options.len().saturating_sub(1));
        }
    }

    pub fn key(&mut self, key: &KeyEvent) -> (Outcome, Vec<Effect>) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => (Outcome::Close, Vec::new()),
            KeyCode::Enter => {
                let Some(o) = self.options.get(self.cursor) else {
                    return (Outcome::Stay, Vec::new());
                };
                let fx = if o.target == OpenTarget::Editor {
                    vec![Effect::Suspend(crate::suspend::editor_command_from_env(
                        std::path::Path::new(&self.path),
                    ))]
                } else {
                    vec![req(
                        "open:do",
                        Request::OpenIn {
                            target: o.target,
                            path: self.path.clone(),
                        },
                    )]
                };
                (Outcome::Close, fx)
            }
            _ => {
                if let Some(d) = nav(key, 5) {
                    self.cursor = step(self.cursor, self.options.len(), d);
                }
                (Outcome::Stay, Vec::new())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn k(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn picks_a_target_and_runs_the_editor_in_the_terminal() {
        let mut ui = OpenInUi::default();
        let fx = ui.open("/p/file.rs");
        assert!(matches!(&fx[..], [Effect::Req { key, .. }] if key == "open:list"));
        ui.on_reply(
            "open:list",
            Ok(Response::OpenTargets(vec![
                OpenOption {
                    target: OpenTarget::Vscode,
                    label: "VS Code".into(),
                },
                OpenOption {
                    target: OpenTarget::Editor,
                    label: "$EDITOR".into(),
                },
            ])),
        );
        let (out, fx) = ui.key(&k(KeyCode::Enter));
        assert_eq!(out, Outcome::Close);
        assert!(
            matches!(&fx[..], [Effect::Req { req: Request::OpenIn { target: OpenTarget::Vscode, path }, .. }] if path == "/p/file.rs")
        );
        ui.key(&k(KeyCode::Down));
        let (_, fx) = ui.key(&k(KeyCode::Enter));
        assert!(
            matches!(&fx[..], [Effect::Suspend(c)] if c.args.last().map(String::as_str) == Some("/p/file.rs"))
        );
        let (out, _) = ui.key(&k(KeyCode::Esc));
        assert_eq!(out, Outcome::Close);
    }
}
