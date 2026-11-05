use super::*;
use as_core::ProviderId;
use as_core::sessions::{ClaudeProfile, SessionSummary};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(crate) fn agent(id: &str, name: &str, dir: &str, status: AgentStatus) -> AgentState {
    let mut a = AgentState::default();
    a.config.id = id.into();
    a.config.name = name.into();
    a.config.project_dir = dir.into();
    a.config.provider = ProviderId::Claude;
    a.config.model = "sonnet".into();
    // Earlier letters are newer, so "a" sorts first in the sidebar.
    let day = 28 - u32::from(id.bytes().next().unwrap_or(b'a') - b'a').min(20);
    a.config.created_at = format!("2026-01-{day:02}T00:00:00Z");
    a.status = status;
    a
}

fn app_with(agents: Vec<AgentState>) -> App {
    let mut app = App::new(
        AppConfig::default(),
        agents,
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    app.on_resize(120, 40);
    app.start();
    app
}

fn two_agents() -> App {
    app_with(vec![
        agent("a", "api", "/p1", AgentStatus::Running),
        agent("bb", "web", "/p2", AgentStatus::Idle),
    ])
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn chr(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

fn ctrl_a() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
}

fn prefix(app: &mut App, c: char) -> Vec<Effect> {
    assert!(app.handle_key(ctrl_a()).is_empty());
    assert!(app.prefix_pending);
    app.handle_key(chr(c))
}

#[test]
fn start_selects_the_first_agent_subscribes_and_resizes() {
    let mut app = App::new(
        AppConfig::default(),
        vec![agent("a", "api", "/p1", AgentStatus::Running)],
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    let mut fx = app.on_resize(120, 40);
    fx.extend(app.start());
    assert_eq!(app.selected.as_deref(), Some("a"));
    assert!(fx.contains(&Effect::SetFocus(true)));
    assert!(fx.contains(&Effect::Subscribe("a".into())));
    let (cols, rows) = app.pane_size();
    assert!(fx.contains(&Effect::Resize("a".into(), cols, rows)));
    // Terminal takes the screen minus header, footer, sidebar, title.
    assert_eq!((cols, rows), (120 - 28, 40 - 3));
}

#[test]
fn keys_go_to_the_pty_in_terminal_focus_except_the_prefix() {
    let mut app = two_agents();
    assert_eq!(app.focus, Focus::Terminal);
    let fx = app.handle_key(chr('h'));
    assert_eq!(fx, vec![Effect::SendRaw("a".into(), b"h".to_vec())]);
    let fx = app.handle_key(key(KeyCode::Enter));
    assert_eq!(fx, vec![Effect::SendRaw("a".into(), b"\r".to_vec())]);
    // Ctrl+C is not special.
    let fx = app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(fx, vec![Effect::SendRaw("a".into(), vec![3])]);
}

#[test]
fn prefix_transitions() {
    let mut app = two_agents();
    assert!(app.handle_key(ctrl_a()).is_empty());
    assert!(app.prefix_pending);
    // Unknown key cancels without sending anything.
    assert!(app.handle_key(chr('z')).is_empty());
    assert!(!app.prefix_pending);
    // prefix a / prefix prefix send a literal Ctrl+A.
    assert_eq!(
        prefix(&mut app, 'a'),
        vec![Effect::SendRaw("a".into(), vec![1])]
    );
    assert!(app.handle_key(ctrl_a()).is_empty());
    assert_eq!(
        app.handle_key(ctrl_a()),
        vec![Effect::SendRaw("a".into(), vec![1])]
    );
    // prefix n opens the dialog.
    prefix(&mut app, 'n');
    assert!(matches!(app.modal, Some(Modal::NewAgent(_))));
}

#[test]
fn prefix_commands_kill_restart_yolo() {
    let mut app = two_agents();
    assert_eq!(prefix(&mut app, 'x'), vec![Effect::Kill("a".into())]);
    assert_eq!(prefix(&mut app, 'R'), vec![Effect::Restart("a".into())]);
    assert_eq!(
        prefix(&mut app, 'y'),
        vec![Effect::ToggleYolo("a".into(), true)]
    );
    assert_eq!(prefix(&mut app, 'Y'), vec![Effect::SetGlobalYolo(true)]);
    prefix(&mut app, 'd');
    assert!(matches!(&app.modal, Some(Modal::ConfirmRemove { agent_id, .. }) if agent_id == "a"));
    assert_eq!(app.handle_key(chr('y')), vec![Effect::Remove("a".into())]);
    assert!(app.modal.is_none());
}

#[test]
fn remove_can_be_cancelled_and_rename_submits() {
    let mut app = two_agents();
    prefix(&mut app, 'd');
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(app.modal.is_none());
    prefix(&mut app, 'r');
    assert!(matches!(&app.modal, Some(Modal::Rename { input, .. }) if input.text() == "api"));
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Backspace));
    }
    for c in "core".chars() {
        app.handle_key(chr(c));
    }
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        vec![Effect::Rename("a".into(), "core".into())]
    );
    assert!(app.modal.is_none());
}

#[test]
fn jumping_and_stepping_follow_sidebar_order() {
    let mut app = two_agents();
    // Sidebar order: groups by activity then path: /p1 (api) is older? both have no activity:
    // order is by creation time of each group's newest agent: "bb" is created later -> /p2 first.
    let order: Vec<_> = crate::sidebar::agent_order(&app.agents, "")
        .into_iter()
        .map(|i| app.agents[i].id.clone())
        .collect();
    let first = order[0].clone();
    let second = order[1].clone();
    let fx = prefix(&mut app, '2');
    assert_eq!(app.selected.as_deref(), Some(second.as_str()));
    assert!(fx.contains(&Effect::Subscribe(second.clone())));
    prefix(&mut app, '1');
    assert_eq!(app.selected.as_deref(), Some(first.as_str()));
    prefix(&mut app, 'j');
    assert_eq!(app.selected.as_deref(), Some(second.as_str()));
    prefix(&mut app, 'j');
    assert_eq!(app.selected.as_deref(), Some(first.as_str()), "wraps");
    prefix(&mut app, 'k');
    assert_eq!(app.selected.as_deref(), Some(second.as_str()));
    assert!(prefix(&mut app, '9').is_empty(), "out of range");
}

#[test]
fn sidebar_focus_navigation_and_filter() {
    let mut app = two_agents();
    prefix(&mut app, 's');
    assert_eq!(app.focus, Focus::App);
    // Cursor over rows including headers: moving down from the selected agent eventually reaches the other.
    for _ in 0..4 {
        app.handle_key(chr('j'));
    }
    let selected_after = app.selected.clone();
    assert!(selected_after.is_some());
    // Enter attaches (terminal focus).
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Terminal);
    // Filter.
    prefix(&mut app, 's');
    app.handle_key(chr('/'));
    assert!(app.sidebar.filtering);
    for c in "web".chars() {
        app.handle_key(chr(c));
    }
    assert_eq!(app.sidebar.filter, "web");
    assert_eq!(app.sidebar_rows().len(), 2, "one header + one agent");
    app.handle_key(key(KeyCode::Enter));
    assert!(!app.sidebar.filtering);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Focus::Terminal);
}

#[test]
fn collapsing_a_project_hides_its_agents() {
    let mut app = two_agents();
    prefix(&mut app, 's');
    app.handle_key(chr(' '));
    assert_eq!(app.sidebar.collapsed.len(), 1);
    assert_eq!(app.sidebar_rows().len(), 3);
    app.handle_key(chr(' '));
    assert!(app.sidebar.collapsed.is_empty());
}

#[test]
fn scroll_mode_enters_and_leaves() {
    let mut app = two_agents();
    // Give the terminal some history.
    {
        let term = app.terms.get_mut("a").unwrap();
        term.sub = SubState::Live;
        for i in 0..100 {
            term.emu.process(format!("line {i}\r\n").as_bytes());
        }
    }
    prefix(&mut app, '[');
    assert!(app.scroll_mode);
    // Typed keys are not sent while scrolling.
    assert!(app.handle_key(chr('x')).is_empty());
    app.handle_key(key(KeyCode::PageUp));
    assert!(app.terms["a"].emu.scrollback() > 10);
    app.handle_key(chr('G'));
    assert_eq!(app.terms["a"].emu.scrollback(), 0);
    app.handle_key(chr('k'));
    assert_eq!(app.terms["a"].emu.scrollback(), 1);
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.scroll_mode);
    assert_eq!(app.terms["a"].emu.scrollback(), 0);
    // Shift+PgUp from terminal focus enters scroll mode.
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT));
    assert!(app.scroll_mode);
}

#[test]
fn input_bar_sends_with_submit_semantics() {
    let mut app = two_agents();
    let fx = prefix(&mut app, 'i');
    assert!(app.input_visible);
    assert_eq!(app.focus, Focus::Input);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::Resize(..))),
        "pane shrank"
    );
    for c in "hello".chars() {
        app.handle_key(chr(c));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    for c in "world".chars() {
        app.handle_key(chr(c));
    }
    assert_eq!(app.input.line_count(), 2);
    let fx = app.handle_key(key(KeyCode::Enter));
    assert!(fx.contains(&Effect::SendInput("a".into(), "hello\nworld".into())));
    assert!(app.input.is_empty());
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Focus::Terminal);
    assert!(app.input_visible);
    prefix(&mut app, 'i');
    assert!(!app.input_visible);
}

#[test]
fn paste_is_bracketed_only_when_the_app_asked_for_it() {
    let mut app = two_agents();
    app.terms.get_mut("a").unwrap().sub = SubState::Live;
    assert_eq!(
        app.handle_paste("a\nb"),
        vec![Effect::SendRaw("a".into(), b"a\rb".to_vec())]
    );
    app.terms.get_mut("a").unwrap().emu.process(b"\x1b[?2004h");
    assert_eq!(
        app.handle_paste("a\nb"),
        vec![Effect::SendRaw(
            "a".into(),
            b"\x1b[200~a\nb\x1b[201~".to_vec()
        )]
    );
}

#[test]
fn quit_decision_logic() {
    let idle = vec![
        agent("a", "x", "/p", AgentStatus::Idle),
        agent("b", "y", "/p", AgentStatus::Errored),
    ];
    assert_eq!(quit_decision(&idle), QuitDecision::Exit);
    assert_eq!(quit_decision(&[]), QuitDecision::Exit);
    let busy = vec![
        agent("a", "x", "/p", AgentStatus::Running),
        agent("b", "y", "/p", AgentStatus::Starting),
        agent("c", "z", "/p", AgentStatus::Idle),
    ];
    assert_eq!(quit_decision(&busy), QuitDecision::Confirm { running: 2 });
}

#[test]
fn quitting_with_nothing_running_detaches_immediately() {
    let mut app = app_with(vec![agent("a", "x", "/p", AgentStatus::Idle)]);
    assert_eq!(prefix(&mut app, 'q'), vec![Effect::Quit]);
    assert!(app.quit);
}

#[test]
fn quit_dialog_detach_stop_and_cancel() {
    let mut app = two_agents();
    prefix(&mut app, 'q');
    assert!(matches!(
        app.modal,
        Some(Modal::Quit {
            running: 1,
            choice: QuitChoice::Detach,
            ..
        })
    ));
    // Cancel with Esc.
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(app.modal.is_none());
    // Enter on the default choice = detach.
    prefix(&mut app, 'q');
    assert_eq!(app.handle_key(key(KeyCode::Enter)), vec![Effect::Quit]);

    // Stop all: arrows + Enter, then wait for the daemon.
    let mut app = two_agents();
    prefix(&mut app, 'q');
    app.handle_key(key(KeyCode::Right));
    assert!(matches!(
        app.modal,
        Some(Modal::Quit {
            choice: QuitChoice::StopAll,
            ..
        })
    ));
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        vec![Effect::ShutdownAndQuit]
    );
    assert!(matches!(
        app.modal,
        Some(Modal::Quit { stopping: true, .. })
    ));
    assert!(
        app.handle_key(chr('c')).is_empty(),
        "no cancelling while stopping"
    );
    assert!(app.modal.is_some());
    assert_eq!(
        app.handle_msg(Msg::ShutdownDone(Ok(()))),
        vec![Effect::Quit]
    );

    // A failed shutdown closes the dialog with an error toast.
    let mut app = two_agents();
    prefix(&mut app, 'q');
    app.handle_key(chr('s'));
    app.handle_msg(Msg::ShutdownDone(Err("boom".into())));
    assert!(app.modal.is_none() && !app.quit);
    assert_eq!(app.toasts.len(), 1);
}

#[test]
fn waiting_state_lifecycle_and_title() {
    let mut app = two_agents();
    assert_eq!(app.title(), "Agent Studio");
    app.handle_event(Event::AgentWaiting {
        agent_id: "a".into(),
    });
    assert!(app.is_waiting(&app.agents[0].clone()));
    assert_eq!(app.waiting_count(), 1);
    assert_eq!(app.title(), "(1) Agent Studio");
    // Output clears it.
    app.terms.get_mut("a").unwrap().sub = SubState::Live;
    app.handle_event(Event::AgentOutput {
        agent_id: "a".into(),
        data: b"x".to_vec(),
    });
    assert_eq!(app.waiting_count(), 0);
    // Typing clears it too.
    app.handle_event(Event::AgentWaiting {
        agent_id: "a".into(),
    });
    app.handle_key(chr('y'));
    assert_eq!(app.waiting_count(), 0);
    // Only running agents wait; unknown agents are ignored.
    app.handle_event(Event::AgentWaiting {
        agent_id: "bb".into(),
    });
    assert_eq!(app.waiting_count(), 0, "bb is idle");
    app.handle_event(Event::AgentWaiting {
        agent_id: "zz".into(),
    });
    assert!(!app.waiting.contains("zz"));
}

#[test]
fn a_restart_elsewhere_blanks_the_agents_screen() {
    let mut app = two_agents();
    app.terms.get_mut("a").unwrap().sub = SubState::Live;
    app.handle_event(Event::AgentOutput {
        agent_id: "a".into(),
        data: b"old\r\nsession\r\ntext".to_vec(),
    });
    assert_eq!(app.terms["a"].emu.content_rows(), 3);
    app.handle_event(Event::AgentRestarted {
        agent_id: "a".into(),
    });
    assert_eq!(app.terms["a"].emu.content_rows(), 1, "blank again");
    assert_eq!(app.terms["a"].emu.cursor_raw(), (0, 0));
    app.handle_event(Event::AgentOutput {
        agent_id: "a".into(),
        data: b"new".to_vec(),
    });
    assert_eq!(app.terms["a"].emu.cursor_raw(), (3, 0));
    // Unknown agents are ignored.
    app.handle_event(Event::AgentRestarted {
        agent_id: "zz".into(),
    });
}

#[test]
fn status_events_update_agents_and_clear_waiting() {
    let mut app = two_agents();
    app.handle_event(Event::AgentWaiting {
        agent_id: "a".into(),
    });
    app.handle_event(Event::AgentStatus {
        agent_id: "a".into(),
        status: AgentStatus::Errored,
        session_id: Some("s1".into()),
        model: "opus".into(),
    });
    let a = app.agent("a").unwrap();
    assert_eq!(
        (a.status, a.model.as_str(), a.session_id.as_deref()),
        (AgentStatus::Errored, "opus", Some("s1"))
    );
    assert!(app.waiting.is_empty());
    let fx = app.handle_event(Event::AgentStatus {
        agent_id: "unknown".into(),
        status: AgentStatus::Running,
        session_id: None,
        model: "x".into(),
    });
    assert_eq!(fx, vec![Effect::Refresh]);
}

#[test]
fn notifications_become_toasts_and_waiting_rings_the_bell() {
    let mut app = two_agents();
    let n = |kind, title: &str| StudioNotification {
        id: format!("n-{title}"),
        kind,
        title: title.into(),
        body: "b".into(),
        agent_id: Some("a".into()),
        timestamp: String::new(),
    };
    let fx = app.handle_event(Event::Notification(n(NotificationKind::AgentWaiting, "w")));
    assert_eq!(fx, vec![Effect::Bell]);
    assert_eq!(app.toasts[0].kind, ToastKind::Warn);
    let fx = app.handle_event(Event::Notification(n(NotificationKind::AgentErrored, "e")));
    assert!(fx.is_empty());
    assert_eq!(app.toasts[1].kind, ToastKind::Error);
    // Sounds off: no bell.
    app.config.enable_sound_effects = false;
    assert!(
        app.handle_event(Event::Notification(n(NotificationKind::AgentWaiting, "w2")))
            .is_empty()
    );
    // Cap and expiry.
    for i in 0..10 {
        app.toast(ToastKind::Info, &format!("t{i}"), "");
    }
    assert_eq!(app.toasts.len(), MAX_TOASTS);
    let later = app.now + TOAST_TTL + Duration::from_millis(1);
    app.tick(later);
    assert!(app.toasts.is_empty());
    // prefix Esc clears.
    app.toast(ToastKind::Info, "x", "");
    prefix(&mut app, '\u{1b}');
}

#[test]
fn prefix_escape_clears_toasts() {
    let mut app = two_agents();
    app.toast(ToastKind::Info, "x", "");
    app.handle_key(ctrl_a());
    app.handle_key(key(KeyCode::Esc));
    assert!(app.toasts.is_empty());
}

#[test]
fn agent_list_refresh_prunes_state_and_reselects() {
    let mut app = two_agents();
    app.waiting.insert("a".into());
    let fx = app.handle_msg(Msg::Agents(vec![agent(
        "bb",
        "web",
        "/p2",
        AgentStatus::Idle,
    )]));
    assert_eq!(app.selected.as_deref(), Some("bb"));
    assert!(app.waiting.is_empty());
    assert!(!app.terms.contains_key("a"));
    assert!(fx.contains(&Effect::Subscribe("bb".into())));
    let fx = app.handle_msg(Msg::Agents(Vec::new()));
    assert_eq!(app.selected, None);
    assert!(fx.is_empty());
    assert!(app.terms.is_empty());
}

#[test]
fn at_most_ten_terminals_stay_subscribed() {
    let agents: Vec<_> = (0..12)
        .map(|i| {
            agent(
                &format!("id{i:02}"),
                &format!("n{i}"),
                "/p",
                AgentStatus::Idle,
            )
        })
        .collect();
    let mut app = app_with(agents);
    let mut unsubscribed = Vec::new();
    for i in 0..12 {
        for e in app.select(&format!("id{i:02}")) {
            if let Effect::Unsubscribe(id) = e {
                unsubscribed.push(id);
            }
        }
    }
    assert_eq!(app.terms.len(), MAX_LIVE_TERMS);
    assert_eq!(unsubscribed.len(), 2);
    assert!(app.terms.contains_key("id11"), "selected one is kept");
    assert!(!unsubscribed.contains(&"id11".to_string()));
}

#[test]
fn output_before_the_buffer_is_held_back_and_replayed_in_order() {
    let mut app = two_agents();
    assert!(matches!(app.terms["a"].sub, SubState::Requested(_)));
    app.handle_event(Event::AgentOutput {
        agent_id: "a".into(),
        data: b"LIVE".to_vec(),
    });
    let fx = app.handle_msg(Msg::Subscribed {
        agent_id: "a".into(),
        result: Ok(b"OLD ".to_vec()),
    });
    assert!(fx.is_empty());
    let text = app.terms["a"].emu_contents();
    assert!(text.starts_with("OLD LIVE"), "{text:?}");
    assert!(matches!(app.terms["a"].sub, SubState::Live));
    // A failed subscription is shown, not hidden.
    app.handle_msg(Msg::Subscribed {
        agent_id: "bb".into(),
        result: Err("nope".into()),
    });
    assert!(
        app.terms
            .get("bb")
            .is_none_or(|t| matches!(t.sub, SubState::Failed))
    );
}

impl Term {
    fn emu_contents(&self) -> String {
        let (cols, rows) = self.emu.size();
        let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, cols, rows));
        self.emu.render(
            Rect::new(0, 0, cols, rows),
            &mut buf,
            crate::term::TermPalette {
                fg: ratatui::style::Color::White,
                bg: ratatui::style::Color::Black,
            },
        );
        (0..rows)
            .map(|y| {
                (0..cols)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[test]
fn queries_in_live_output_are_answered_but_not_in_the_replayed_history() {
    let mut app = two_agents();
    let fx = app.handle_msg(Msg::Subscribed {
        agent_id: "a".into(),
        result: Ok(b"\x1b[6n".to_vec()),
    });
    assert!(fx.is_empty(), "stale query in history");
    let fx = app.handle_event(Event::AgentOutput {
        agent_id: "a".into(),
        data: b"\x1b[6n".to_vec(),
    });
    assert_eq!(fx, vec![Effect::SendRaw("a".into(), b"\x1b[1;1R".to_vec())]);
}

#[test]
fn resize_updates_the_emulator_and_tells_the_daemon_once() {
    let mut app = two_agents();
    let fx = app.on_resize(100, 30);
    assert_eq!(app.terms["a"].emu.size(), (72, 27));
    assert_eq!(fx, vec![Effect::Resize("a".into(), 72, 27)]);
    assert!(app.on_resize(100, 30).is_empty(), "unchanged");
    // Widening the sidebar changes the pane.
    let fx = prefix(&mut app, '>');
    assert_eq!(fx, vec![Effect::Resize("a".into(), 70, 27)]);
}

#[test]
fn created_agent_is_selected_and_attached() {
    let mut app = app_with(Vec::new());
    assert_eq!(app.selected, None);
    prefix(&mut app, 'n');
    let new = agent("zz9", "fresh", "/tmp", AgentStatus::Running);
    let fx = app.handle_msg(Msg::Created(Ok(Box::new(new))));
    assert!(app.modal.is_none());
    assert_eq!(app.selected.as_deref(), Some("zz9"));
    assert_eq!(app.focus, Focus::Terminal);
    assert!(fx.contains(&Effect::Subscribe("zz9".into())));
}

#[test]
fn creation_errors_stay_in_the_dialog() {
    let mut app = two_agents();
    prefix(&mut app, 'n');
    let fx = app.handle_key(key(KeyCode::Enter));
    // Default dir is the selected agent's "/p1", which does not exist here.
    assert!(fx.is_empty());
    let Some(Modal::NewAgent(form)) = &app.modal else {
        panic!("dialog closed")
    };
    assert!(
        form.error
            .as_deref()
            .is_some_and(|e| e.contains("does not exist"))
    );

    let dir = tempfile::tempdir().unwrap();
    let mut app = two_agents();
    prefix(&mut app, 'n');
    if let Some(Modal::NewAgent(form)) = &mut app.modal {
        form.dir.set_text(dir.path().to_str().unwrap());
    }
    let fx = app.handle_key(key(KeyCode::Enter));
    assert!(
        matches!(&fx[..], [Effect::CreateAgent(p)] if p.project_dir == dir.path().to_str().unwrap())
    );
    app.handle_msg(Msg::Created(Err("claude CLI not found on PATH".into())));
    let Some(Modal::NewAgent(form)) = &app.modal else {
        panic!("dialog closed")
    };
    assert!(!form.submitting);
    assert_eq!(form.error.as_deref(), Some("claude CLI not found on PATH"));
}

#[test]
fn config_change_swaps_theme_and_prefix() {
    let mut app = two_agents();
    let cfg = AppConfig {
        theme: as_core::ThemeId::Light,
        prefix_key: "ctrl+b".into(),
        ..AppConfig::default()
    };
    app.handle_event(Event::ConfigChanged(Box::new(cfg)));
    assert_eq!(app.theme.id, as_core::ThemeId::Light);
    // Ctrl+A is now an ordinary key for the agent, Ctrl+B is the prefix.
    assert_eq!(
        app.handle_key(ctrl_a()),
        vec![Effect::SendRaw("a".into(), vec![1])]
    );
    assert!(
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL))
            .is_empty()
    );
    assert!(app.prefix_pending);
}

#[test]
fn reconnect_resubscribes_everything() {
    let mut app = two_agents();
    app.terms.get_mut("a").unwrap().sub = SubState::Live;
    app.on_disconnected();
    app.on_disconnected();
    assert_eq!(app.conn, ConnState::Reconnecting { attempt: 2 });
    let fx = app.on_reconnected();
    assert_eq!(app.conn, ConnState::Connected);
    assert!(fx.contains(&Effect::Refresh));
    assert!(fx.contains(&Effect::Subscribe("a".into())));
    assert!(matches!(app.terms["a"].sub, SubState::Requested(_)));
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::Resize(id, ..) if id == "a"))
    );
}

#[test]
fn focus_events_are_reported() {
    let mut app = two_agents();
    assert_eq!(app.on_focus(false), vec![Effect::SetFocus(false)]);
    assert!(!app.terminal_focused);
}

#[test]
fn help_dialog_scrolls_and_closes_with_escape() {
    let mut app = two_agents();
    prefix(&mut app, '?');
    assert!(matches!(app.modal, Some(Modal::Help)));
    app.handle_key(chr('j'));
    app.handle_key(chr('j'));
    assert_eq!(app.help_scroll, 2);
    app.handle_key(chr('k'));
    assert_eq!(app.help_scroll, 1);
    app.handle_key(key(KeyCode::PageDown));
    assert_eq!(app.help_scroll, 11);
    app.handle_key(chr('g'));
    assert_eq!(app.help_scroll, 0);
    app.handle_key(chr('x'));
    assert!(
        matches!(app.modal, Some(Modal::Help)),
        "other keys are ignored"
    );
    app.handle_key(key(KeyCode::Esc));
    assert!(app.modal.is_none());
    prefix(&mut app, '?');
    assert_eq!(app.help_scroll, 0, "reopening starts at the top");
    app.handle_key(chr('q'));
    assert!(app.modal.is_none());
}

#[test]
fn mouse_wheel_scrolls_and_clicks_select() {
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
    let mut app = two_agents();
    {
        let term = app.terms.get_mut("a").unwrap();
        term.sub = SubState::Live;
        for i in 0..100 {
            term.emu.process(format!("l{i}\r\n").as_bytes());
        }
    }
    let wheel = |kind| MouseEvent {
        kind,
        column: 60,
        row: 10,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(wheel(MouseEventKind::ScrollUp));
    assert!(app.scroll_mode);
    assert_eq!(app.terms["a"].emu.scrollback(), 3);
    // Click the first agent row of the sidebar: row 0 is the header, row 1 the first agent.
    let first = crate::sidebar::agent_order(&app.agents, "")[0];
    let id = app.agents[first].id.clone();
    let click = MouseEvent {
        kind: MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
        column: 3,
        row: 2,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(click);
    assert_eq!(app.selected.as_deref(), Some(id.as_str()));
}

// ── sessions, profiles, models (P4) ─────────────────────────────────────────

fn sess(id: &str, provider: ProviderId, project: &str, prompt: &str) -> SessionSummary {
    SessionSummary {
        session_id: id.into(),
        provider,
        project_path: project.into(),
        first_prompt: prompt.into(),
        message_count: 4,
        modified_at: chrono::Utc::now().to_rfc3339(),
        ..Default::default()
    }
}

fn profile(dir: &str, label: &str, is_default: bool) -> ClaudeProfile {
    ClaudeProfile {
        config_dir: dir.into(),
        label: label.into(),
        is_default,
    }
}

fn two_profiles() -> Vec<ClaudeProfile> {
    vec![
        profile("/h/.claude", "default", true),
        profile("/h/.claude-work", "work", false),
    ]
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(chr(c));
    }
}

#[test]
fn prefix_o_opens_the_picker_loads_sessions_and_resumes_the_pick() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_str().unwrap();
    let mut app = two_agents();
    let fx = prefix(&mut app, 'o');
    assert_eq!(
        fx,
        vec![Effect::LoadSessions {
            provider: ProviderId::Claude,
            force: false
        }]
    );
    assert!(matches!(&app.modal, Some(Modal::Resume(p)) if p.loading));
    let fx = app.handle_msg(Msg::Sessions {
        provider: ProviderId::Claude,
        result: Ok(vec![
            sess("s-1", ProviderId::Claude, project, "fix the bug"),
            sess("s-2", ProviderId::Claude, "/p1", "other"),
        ]),
    });
    assert!(fx.is_empty());
    // Enter picks the first (newest first order is the daemon's; both are "now").
    let fx = app.handle_key(key(KeyCode::Enter));
    let [Effect::CreateAgent(p)] = &fx[..] else {
        panic!("{fx:?}")
    };
    assert_eq!(p.resume_session_id.as_deref(), Some("s-1"));
    assert_eq!(p.project_dir, project);
    assert_eq!(p.provider, ProviderId::Claude);
    assert_eq!(p.name, "fix the bug");
    assert_eq!(p.model, "sonnet");
    assert!(
        app.modal.is_none(),
        "closed when the agent is being created"
    );
}

#[test]
fn picker_toggles_hide_reload_and_tab_provider_emit_effects() {
    let mut app = two_agents();
    prefix(&mut app, 'o');
    app.handle_msg(Msg::Sessions {
        provider: ProviderId::Claude,
        result: Ok(vec![sess("s-1", ProviderId::Claude, "/p1", "hello")]),
    });
    assert_eq!(
        app.handle_key(chr('h')),
        vec![Effect::HideSession("s-1".into(), true)]
    );
    assert!(matches!(&app.modal, Some(Modal::Resume(p)) if p.visible_len() == 0));
    assert_eq!(
        app.handle_key(chr('r')),
        vec![Effect::LoadSessions {
            provider: ProviderId::Claude,
            force: true
        }]
    );
    assert_eq!(
        app.handle_key(key(KeyCode::Tab)),
        vec![Effect::LoadSessions {
            provider: ProviderId::Codex,
            force: false
        }]
    );
    // An answer for the provider we left is ignored; Esc closes.
    app.handle_msg(Msg::Sessions {
        provider: ProviderId::Claude,
        result: Ok(vec![sess("x", ProviderId::Claude, "/p1", "late")]),
    });
    assert!(matches!(&app.modal, Some(Modal::Resume(p)) if p.visible_len() == 0 && p.loading));
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(app.modal.is_none());
}

#[test]
fn resuming_into_a_missing_directory_keeps_the_picker_and_toasts() {
    let mut app = two_agents();
    prefix(&mut app, 'o');
    app.handle_msg(Msg::Sessions {
        provider: ProviderId::Claude,
        result: Ok(vec![sess(
            "s-1",
            ProviderId::Claude,
            "/definitely/gone",
            "x",
        )]),
    });
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert!(matches!(app.modal, Some(Modal::Resume(_))));
    assert!(
        app.toasts
            .iter()
            .any(|t| t.body.contains("no longer exists"))
    );
}

#[test]
fn picker_filter_mode_types_into_the_query() {
    let mut app = two_agents();
    prefix(&mut app, 'o');
    app.handle_msg(Msg::Sessions {
        provider: ProviderId::Claude,
        result: Ok(vec![
            sess("s-1", ProviderId::Claude, "/p1", "login form"),
            sess("s-2", ProviderId::Claude, "/p1", "billing docs"),
        ]),
    });
    app.handle_key(chr('/'));
    type_text(&mut app, "bill");
    let Some(Modal::Resume(p)) = &app.modal else {
        panic!()
    };
    assert_eq!(
        p.visible()
            .map(|s| s.session_id.as_str())
            .collect::<Vec<_>>(),
        ["s-2"]
    );
    app.handle_paste(" docs");
    let Some(Modal::Resume(p)) = &app.modal else {
        panic!()
    };
    assert_eq!(p.query.text(), "bill docs");
}

fn open_form_resume(app: &mut App) {
    prefix(app, 'n');
    let Some(Modal::NewAgent(form)) = &mut app.modal else {
        panic!()
    };
    form.field = crate::new_agent::Field::Resume;
}

#[test]
fn the_dialog_resume_field_fills_project_and_provider_and_creates_with_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_str().unwrap();
    let mut app = two_agents();
    open_form_resume(&mut app);
    let fx = app.handle_key(chr(' '));
    assert_eq!(
        fx,
        vec![Effect::LoadSessions {
            provider: ProviderId::Claude,
            force: false
        }]
    );
    app.handle_msg(Msg::Sessions {
        provider: ProviderId::Claude,
        result: Ok(vec![sess(
            "s-9",
            ProviderId::Codex,
            project,
            "port the parser",
        )]),
    });
    // Enter picks (it does not submit the dialog).
    let fx = app.handle_key(key(KeyCode::Enter));
    assert!(
        fx.contains(&Effect::LoadModels(ProviderId::Codex)),
        "{fx:?}"
    );
    let Some(Modal::NewAgent(form)) = &app.modal else {
        panic!("dialog closed")
    };
    assert!(form.picker.is_none());
    assert_eq!(form.provider, ProviderId::Codex, "the session's provider");
    assert_eq!(form.name.text(), "port the parser");
    assert!(
        form.dir
            .text()
            .ends_with(dir.path().file_name().unwrap().to_str().unwrap())
    );
    // Enter creates it.
    let fx = app.handle_key(key(KeyCode::Enter));
    let [Effect::CreateAgent(p)] = &fx[..] else {
        panic!("{fx:?}")
    };
    assert_eq!(p.resume_session_id.as_deref(), Some("s-9"));
    assert_eq!(p.provider, ProviderId::Codex);
    assert_eq!(p.project_dir, project);
}

#[test]
fn changing_provider_drops_a_picked_session_and_backspace_clears_it() {
    let mut app = two_agents();
    open_form_resume(&mut app);
    let Some(Modal::NewAgent(form)) = &mut app.modal else {
        panic!()
    };
    form.apply_session(sess("s-1", ProviderId::Claude, "/p1", "x"));
    assert!(form.resume.is_some());
    form.field = crate::new_agent::Field::Provider;
    app.handle_key(key(KeyCode::Right));
    let Some(Modal::NewAgent(form)) = &mut app.modal else {
        panic!()
    };
    assert!(form.resume.is_none(), "the session belongs to Claude");
    form.apply_session(sess("s-2", ProviderId::Codex, "/p1", "y"));
    form.field = crate::new_agent::Field::Resume;
    app.handle_key(key(KeyCode::Backspace));
    let Some(Modal::NewAgent(form)) = &app.modal else {
        panic!()
    };
    assert!(form.resume.is_none());
}

#[test]
fn the_dialog_picker_esc_returns_to_the_dialog() {
    let mut app = two_agents();
    open_form_resume(&mut app);
    app.handle_key(chr(' '));
    assert!(matches!(&app.modal, Some(Modal::NewAgent(f)) if f.picker.is_some()));
    app.handle_key(key(KeyCode::Esc));
    assert!(matches!(&app.modal, Some(Modal::NewAgent(f)) if f.picker.is_none()));
}

#[test]
fn profile_field_shows_with_several_profiles_and_maps_the_project_on_change() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_str().unwrap().to_string();
    let mut app = two_agents();
    app.config
        .claude_profile_by_project
        .insert(project.clone(), "/h/.claude-work".into());
    // One profile: no field.
    app.handle_msg(Msg::Profiles(vec![profile("/h/.claude", "default", true)]));
    prefix(&mut app, 'n');
    let Some(Modal::NewAgent(form)) = &mut app.modal else {
        panic!()
    };
    assert!(!form.fields().contains(&crate::new_agent::Field::Profile));
    form.dir.set_text(&project);

    // Two profiles: shown, defaulting to the project's mapping.
    app.handle_msg(Msg::Profiles(two_profiles()));
    let Some(Modal::NewAgent(form)) = &mut app.modal else {
        panic!()
    };
    form.set_profiles(two_profiles());
    assert!(form.fields().contains(&crate::new_agent::Field::Profile));
    assert_eq!(form.profile_idx, 1, "work");
    // Unchanged -> plain create.
    let fx = app.handle_key(key(KeyCode::Enter));
    assert!(matches!(&fx[..], [Effect::CreateAgent(_)]), "{fx:?}");

    // Pick the default profile -> map first, then create.
    let mut app = two_agents();
    app.handle_msg(Msg::Profiles(two_profiles()));
    prefix(&mut app, 'n');
    let Some(Modal::NewAgent(form)) = &mut app.modal else {
        panic!()
    };
    form.dir.set_text(&project);
    form.field = crate::new_agent::Field::Profile;
    app.handle_key(key(KeyCode::Right));
    let fx = app.handle_key(key(KeyCode::Enter));
    let [
        Effect::CreateAgentWithProfile {
            project_dir,
            config_dir,
            payload,
        },
    ] = &fx[..]
    else {
        panic!("{fx:?}")
    };
    assert_eq!(project_dir, &project);
    assert_eq!(config_dir, "/h/.claude-work");
    assert_eq!(payload.project_dir, project);
}

#[test]
fn a_resumed_session_never_changes_the_project_profile() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_str().unwrap();
    let mut app = two_agents();
    app.handle_msg(Msg::Profiles(two_profiles()));
    prefix(&mut app, 'n');
    let Some(Modal::NewAgent(form)) = &mut app.modal else {
        panic!()
    };
    form.field = crate::new_agent::Field::Profile;
    form.profile_idx = 1;
    form.apply_session(sess("s-1", ProviderId::Claude, project, "x"));
    let fx = app.handle_key(key(KeyCode::Enter));
    assert!(matches!(&fx[..], [Effect::CreateAgent(_)]), "{fx:?}");
}

#[test]
fn prefix_p_chooses_a_profile_for_the_project() {
    let mut app = two_agents();
    app.handle_msg(Msg::Profiles(two_profiles()));
    app.config
        .claude_profile_by_project
        .insert("/p1".into(), "/h/.claude-work".into());
    let fx = prefix(&mut app, 'P');
    assert_eq!(fx, vec![Effect::LoadProfiles]);
    let Some(Modal::Profile(c)) = &app.modal else {
        panic!()
    };
    assert_eq!(c.project_dir, "/p1");
    assert_eq!(c.choices.len(), 3);
    assert_eq!(c.idx, 2, "the current mapping is preselected");
    app.handle_key(key(KeyCode::Up));
    let fx = app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        fx,
        vec![Effect::SetProjectProfile {
            project_dir: "/p1".into(),
            config_dir: Some(String::new())
        }]
    );
    assert!(app.modal.is_none());
    // The "no mapping" entry removes it.
    prefix(&mut app, 'P');
    app.handle_key(key(KeyCode::Down)); // wraps from the last entry to the first
    let fx = app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        fx,
        vec![Effect::SetProjectProfile {
            project_dir: "/p1".into(),
            config_dir: None
        }]
    );
}

#[test]
fn prefix_p_in_sidebar_targets_the_header_under_the_cursor() {
    let mut app = two_agents();
    app.handle_key(ctrl_a());
    app.handle_key(chr('s'));
    app.sidebar.cursor = Some(RowKey::Header("/p2".into()));
    app.handle_key(chr('P'));
    assert!(matches!(&app.modal, Some(Modal::Profile(c)) if c.project_dir == "/p2"));
}

#[test]
fn profile_chip_only_for_non_default_profiles() {
    let mut app = two_agents();
    app.profiles = two_profiles();
    app.config
        .claude_profile_by_project
        .insert("/p1".into(), "/h/.claude-work".into());
    app.config
        .claude_profile_by_project
        .insert("/p2".into(), String::new());
    app.config
        .claude_profile_by_project
        .insert("/p3".into(), "/h/.claude".into());
    assert_eq!(app.profile_chip("/p1/sub").as_deref(), Some("work"));
    assert_eq!(app.profile_chip("/p2"), None, "explicit default");
    assert_eq!(app.profile_chip("/p3"), None, "default by path");
    assert_eq!(app.profile_chip("/other"), None);
    app.config
        .claude_profile_by_project
        .insert("/p4".into(), "/gone".into());
    assert_eq!(
        app.profile_chip("/p4").as_deref(),
        Some("/gone"),
        "missing profile: raw path"
    );
}

#[test]
fn models_come_from_the_daemon_and_follow_the_provider() {
    let mut app = two_agents();
    let fx = prefix(&mut app, 'n');
    assert!(
        fx.contains(&Effect::LoadModels(ProviderId::Claude)),
        "{fx:?}"
    );
    let discovered = vec![
        ProviderModelOption {
            id: "opus-9".into(),
            label: "Opus 9".into(),
            ..Default::default()
        },
        ProviderModelOption {
            id: "sonnet".into(),
            label: "Sonnet".into(),
            ..Default::default()
        },
    ];
    app.handle_msg(Msg::Models {
        provider: ProviderId::Claude,
        models: discovered.clone(),
    });
    let Some(Modal::NewAgent(form)) = &mut app.modal else {
        panic!()
    };
    assert_eq!(form.model_options().len(), 2);
    assert_eq!(form.model(), "sonnet", "the preferred model stays selected");
    // Another provider asks for its own list; a stale answer is ignored.
    form.field = crate::new_agent::Field::Provider;
    let fx = app.handle_key(key(KeyCode::Right));
    assert_eq!(fx, vec![Effect::LoadModels(ProviderId::Codex)]);
    app.handle_msg(Msg::Models {
        provider: ProviderId::Claude,
        models: discovered,
    });
    let Some(Modal::NewAgent(form)) = &app.modal else {
        panic!()
    };
    assert_eq!(form.provider, ProviderId::Codex);
    assert!(form.model_options().iter().all(|m| m.id.starts_with("gpt")));
    // A reopened dialog uses the cache right away.
    app.modal = None;
    let fx = prefix(&mut app, 'n');
    assert!(fx.contains(&Effect::LoadModels(ProviderId::Claude)));
    let Some(Modal::NewAgent(form)) = &app.modal else {
        panic!()
    };
    assert_eq!(form.model_options()[0].id, "opus-9");
}

// ── P10: modal prefix, header clicks, update banner ─────────────────────────

use crate::keymap::Command as Cmd;
use as_core::updates::{CheckStatus, InstallMethod, UpdateInfo, UpdateState};

fn update_state(status: CheckStatus) -> UpdateState {
    let available = status == CheckStatus::Available;
    UpdateState {
        enabled: true,
        current_version: "0.1.0".into(),
        status,
        checked_at_ms: Some(1),
        latest: available.then(|| UpdateInfo {
            version: "0.2.0".into(),
            tag: "v0.2.0".into(),
            url: "https://example.test/r".into(),
            notes: String::new(),
            asset_url: None,
        }),
        install_method: InstallMethod::Homebrew,
        hint: if available {
            "Run `brew upgrade agent-studio`".into()
        } else {
            String::new()
        },
    }
}

#[test]
fn prefix_commands_work_inside_the_panels_modals() {
    // Terminals toggle: the panel gives way.
    let mut app = two_agents();
    app.exec(Cmd::Settings);
    assert!(matches!(app.modal, Some(Modal::Settings)));
    assert!(app.handle_key(ctrl_a()).is_empty());
    assert!(app.prefix_pending);
    assert!(matches!(app.modal, Some(Modal::Settings)), "still open");
    app.handle_key(chr('t'));
    assert!(!app.prefix_pending);
    assert!(app.modal.is_none());
    assert!(app.free_terms.open, "the terminals panel opened");

    // Palette and help replace the panel.
    let mut app = two_agents();
    app.exec(Cmd::GitPanel);
    prefix(&mut app, 'p');
    assert!(matches!(app.modal, Some(Modal::Palette(_))));
    let mut app = two_agents();
    app.exec(Cmd::ToggleFiles);
    prefix(&mut app, '?');
    assert!(matches!(app.modal, Some(Modal::Help)));

    // Close.
    let mut app = two_agents();
    app.exec(Cmd::Settings);
    prefix_key(&mut app, key(KeyCode::Esc));
    assert!(app.modal.is_none());
}

fn prefix_key(app: &mut App, k: KeyEvent) -> Vec<Effect> {
    app.handle_key(ctrl_a());
    app.handle_key(k)
}

#[test]
fn other_prefix_commands_are_dropped_inside_a_modal() {
    let mut app = two_agents();
    app.exec(Cmd::Settings);
    // `n` (new agent) is not on the allow-list: nothing happens, the prefix is spent.
    let fx = prefix(&mut app, 'n');
    assert!(fx.is_empty());
    assert!(matches!(app.modal, Some(Modal::Settings)));
    assert!(!app.prefix_pending);
    // Unknown key after the prefix: same.
    prefix(&mut app, 'w');
    assert!(matches!(app.modal, Some(Modal::Settings)));
    // `q` still closes the panel afterwards (the key was not eaten as a panel key either).
    app.handle_key(chr('q'));
    assert!(app.modal.is_none());
}

#[test]
fn forms_and_text_entry_keep_ctrl_a_for_themselves() {
    // The new agent form is not a panel.
    let mut app = two_agents();
    app.exec(Cmd::NewAgent);
    app.handle_key(ctrl_a());
    assert!(!app.prefix_pending);
    assert!(matches!(app.modal, Some(Modal::NewAgent(_))));
    // The file search takes typed text.
    let mut app = two_agents();
    app.exec(Cmd::FileSearch);
    assert!(app.files.search.active);
    app.handle_key(ctrl_a());
    assert!(!app.prefix_pending);
    assert!(matches!(app.modal, Some(Modal::Files)));
}

#[test]
fn clicking_a_header_tab_switches_the_view() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = two_agents();
    let header = app.layout().header;
    let spans = crate::ui::header::tab_spans(header);
    let click = |col: u16, row: u16| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let (_, grid_x, _) = spans[1];
    app.handle_mouse(click(grid_x + 1, 0));
    assert_eq!(app.view, ViewMode::Grid);
    let (_, overview_x, w) = spans[2];
    app.handle_mouse(click(overview_x + w - 1, 0));
    assert_eq!(app.view, ViewMode::Overview);
    let (_, chat_x, _) = spans[0];
    app.handle_mouse(click(chat_x, 0));
    assert_eq!(app.view, ViewMode::Chat);
    // Between the title chip and the tabs, and past the last tab: nothing.
    app.handle_mouse(click(2, 0));
    assert_eq!(app.view, ViewMode::Chat);
    app.handle_mouse(click(overview_x + w + 3, 0));
    assert_eq!(app.view, ViewMode::Chat);
    // A modal owns the screen.
    app.exec(Cmd::Help);
    app.handle_mouse(click(grid_x + 1, 0));
    assert_eq!(app.view, ViewMode::Chat);
}

#[test]
fn tab_hit_testing_matches_the_drawn_header() {
    // The spans start right after the title chip and tile without gaps.
    let area = Rect::new(0, 0, 100, 1);
    let spans = crate::ui::header::tab_spans(area);
    assert_eq!(spans[0].1, 15);
    assert_eq!(spans[1].1, spans[0].1 + spans[0].2);
    assert_eq!(crate::ui::header::tab_at(area, 14), None);
    assert_eq!(crate::ui::header::tab_at(area, 15), Some(ViewMode::Chat));
    assert_eq!(crate::ui::header::tab_at(area, 200), None);
}

#[test]
fn update_banner_shows_dismisses_and_comes_back_on_a_manual_check() {
    let mut app = two_agents();
    assert!(app.update_banner().is_none());
    app.handle_event(Event::UpdateAvailable(Box::new(update_state(
        CheckStatus::Available,
    ))));
    assert_eq!(
        app.update_banner().map(|i| i.version.as_str()),
        Some("0.2.0")
    );
    assert!(app.update_hint().contains("brew upgrade"));
    // Esc after the prefix dismisses it, for this version, and the choice is saved.
    prefix_key(&mut app, key(KeyCode::Esc));
    assert!(app.update_banner().is_none());
    assert_eq!(
        app.state_snapshot().dismissed_update.as_deref(),
        Some("0.2.0")
    );
    // The same version announced again stays dismissed.
    app.handle_event(Event::UpdateAvailable(Box::new(update_state(
        CheckStatus::Available,
    ))));
    assert!(app.update_banner().is_none());
    // A restart restores the dismissal.
    let saved = app.state_snapshot();
    let mut fresh = two_agents();
    fresh.restore_state(saved);
    fresh.handle_event(Event::UpdateAvailable(Box::new(update_state(
        CheckStatus::Available,
    ))));
    assert!(fresh.update_banner().is_none());
    // A manual check shows it again and toasts the result.
    app.handle_msg(Msg::Reply {
        key: "upd:check".into(),
        result: Ok(as_proto::Response::UpdateState(Box::new(update_state(
            CheckStatus::Available,
        )))),
    });
    assert!(app.update_banner().is_some());
    assert!(app.toasts.iter().any(|t| t.body.contains("0.2.0")));
}

#[test]
fn manual_update_check_requests_and_reports() {
    let mut app = two_agents();
    let fx = app.exec(Cmd::CheckUpdates);
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Req { key, req: as_proto::Request::CheckForUpdates } if key == "upd:check"
    )));
    let n = app.toasts.len();
    app.handle_msg(Msg::Reply {
        key: "upd:check".into(),
        result: Ok(as_proto::Response::UpdateState(Box::new(update_state(
            CheckStatus::UpToDate,
        )))),
    });
    assert_eq!(app.toasts.len(), n + 1);
    assert!(app.toasts.last().unwrap().body.contains("up to date"));
    assert!(app.update_banner().is_none());
    // The prefix key is the catalog's default.
    let fx = prefix(&mut app, 'U');
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::Req { key, .. } if key == "upd:check"))
    );
    // An older daemon (no such request) is silent for the startup read.
    let n = app.toasts.len();
    app.handle_msg(Msg::Reply {
        key: "upd:state".into(),
        result: Err("unsupported request".into()),
    });
    assert_eq!(app.toasts.len(), n);
}

#[test]
fn start_asks_for_the_update_state() {
    let mut app = App::new(
        AppConfig::default(),
        Vec::new(),
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    app.on_resize(120, 40);
    let fx = app.start();
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Req { key, req: as_proto::Request::GetUpdateState } if key == "upd:state"
    )));
}
