use super::*;
use as_core::ProviderId;
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
fn help_dialog_closes_on_any_key() {
    let mut app = two_agents();
    prefix(&mut app, '?');
    assert!(matches!(app.modal, Some(Modal::Help)));
    app.handle_key(chr('x'));
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
