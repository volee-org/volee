use super::*;
use ratatui::crossterm::event::KeyEventKind;

fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new_with_kind(code, modifiers, KeyEventKind::Press)
}

fn seq(text: &str) -> KeySeq {
    text.parse().unwrap()
}

#[test]
fn keys_parse_and_display_round_trip() {
    for text in [
        "q",
        "G",
        "?",
        "-",
        "+",
        "'",
        "ctrl-c",
        "alt-x",
        "shift-tab",
        "enter",
        "esc",
        "tab",
        "space",
        "f5",
        "↑",
        "home",
        "pgdn",
        "ctrl--",
    ] {
        let combo: KeyCombo = text.parse().unwrap();
        let shown = combo.to_string();
        assert_eq!(
            shown.parse::<KeyCombo>().unwrap(),
            combo,
            "{text} -> {shown}"
        );
    }
    assert_eq!("up".parse::<KeyCombo>().unwrap().to_string(), "↑");
    assert_eq!("Ctrl-C".parse::<KeyCombo>().unwrap().to_string(), "ctrl-C");
    assert_eq!("shift-g".parse::<KeyCombo>().unwrap(), KeyCombo::char('G'));
    assert_eq!(
        "backtab".parse::<KeyCombo>().unwrap().to_string(),
        "shift-tab"
    );
}

#[test]
fn sequences_parse_written_apart_or_together() {
    assert_eq!(seq("g g"), seq("gg"));
    assert_eq!(seq("g g").to_string(), "gg");
    assert_eq!(seq("g g").short(), "gg");
    assert_eq!(seq("space n").to_string(), "space n");
    assert_eq!(seq("space n").short(), "␣n");
    assert_eq!(seq("space N").short(), "␣N");
    assert_eq!(seq("ctrl-w h").to_string(), "ctrl-w h");
    assert_eq!(seq("ctrl-d").short(), "ctrl-d");
    assert_eq!(seq("q").single(), Some(KeyCombo::char('q')));
    assert_eq!(seq("g g").single(), None);
    for round in ["gg", "space n", "ctrl-w h", "space g s", "enter"] {
        assert_eq!(
            seq(round).to_string().parse::<KeySeq>().unwrap(),
            seq(round)
        );
    }
}

#[test]
fn bad_keys_are_refused() {
    for text in ["", "ctrl-", "f99", "hyper-x", "enterr"] {
        assert!(text.parse::<KeyCombo>().is_err(), "{text}");
    }
    for text in ["", "   ", "a b c d", "enterr x", "gggg"] {
        assert!(text.parse::<KeySeq>().is_err(), "{text}");
    }
}

#[test]
fn events_are_normalized_like_written_keys() {
    assert_eq!(
        KeyCombo::from_event(&event(KeyCode::Char('?'), KeyModifiers::SHIFT)),
        KeyCombo::char('?')
    );
    assert_eq!(
        KeyCombo::from_event(&event(KeyCode::BackTab, KeyModifiers::SHIFT)),
        "shift-tab".parse().unwrap()
    );
    assert_eq!(
        KeyCombo::from_event(&event(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        "ctrl-c".parse().unwrap()
    );
    assert_eq!(
        KeyCombo::from_event(&event(KeyCode::Char('x'), KeyModifiers::SUPER)),
        KeyCombo::char('x')
    );
}

#[test]
fn the_default_table_has_no_conflicts_and_every_action() {
    let keymap = Keymap::default();
    keymap.check().unwrap();
    for action in Action::ALL {
        assert!(
            keymap
                .bindings()
                .iter()
                .any(|binding| binding.action == *action && !binding.keys.is_empty()),
            "{action:?} has no key"
        );
        assert_eq!(action.name().parse::<Action>().unwrap(), *action);
    }
    for context in Context::ALL {
        assert_eq!(context.name().parse::<Context>().unwrap(), *context);
        assert!(
            keymap.bindings().iter().any(|b| b.context == *context),
            "{context:?} has no binding"
        );
    }
    for binding in keymap.bindings() {
        assert!(
            binding.label.chars().count() <= 16,
            "label too long for the popups: {}",
            binding.label
        );
    }
    assert_eq!(keymap.leave_key(), "ctrl-q".parse().unwrap());
}

#[test]
fn the_focused_panel_takes_precedence_over_global_keys() {
    let keymap = Keymap::default();
    let contexts = [Context::Help];
    assert_eq!(
        keymap.action(&contexts, KeyCombo::char('q')),
        Some(Action::Back)
    );
    let contexts = [Context::Agents, Context::Global];
    assert_eq!(
        keymap.action(&contexts, KeyCombo::char('q')),
        Some(Action::Quit)
    );
    assert_eq!(
        keymap.action(&contexts, KeyCombo::char('j')),
        Some(Action::Down)
    );
    // The zoom keys reach the view beside the list.
    assert_eq!(
        keymap.action(&contexts, KeyCombo::char('+')),
        Some(Action::ZoomIn)
    );
    let contexts = [Context::Hierarchy, Context::Global];
    assert_eq!(
        keymap.action(&contexts, KeyCombo::char('=')),
        Some(Action::ZoomIn)
    );
    assert_eq!(
        keymap.action(&contexts, KeyCombo::char('-')),
        Some(Action::JumpBack)
    );
}

#[test]
fn sequences_wait_for_their_next_key() {
    let keymap = Keymap::default();
    let contexts = [Context::Agents, Context::Global];
    let g = KeyCombo::char('g');
    let space = KeyCombo::char(' ');
    assert_eq!(keymap.resolve(&contexts, &[], g), Resolved::Prefix);
    assert_eq!(
        keymap.resolve(&contexts, &[g], g),
        Resolved::Action(Context::Agents, Action::First)
    );
    assert_eq!(keymap.resolve(&contexts, &[], space), Resolved::Prefix);
    assert_eq!(
        keymap.resolve(&contexts, &[space], KeyCombo::char('n')),
        Resolved::Action(Context::Global, Action::NewAgent)
    );
    assert_eq!(
        keymap.resolve(&contexts, &[space], KeyCombo::char('x')),
        Resolved::Unbound
    );
    // A single key never waits.
    assert_eq!(
        keymap.resolve(&contexts, &[], KeyCombo::char('n')),
        Resolved::Action(Context::Global, Action::NextMatch)
    );
    // In the tools menu, g opens the git tool at once.
    assert_eq!(
        keymap.resolve(&[Context::Tools], &[], g),
        Resolved::Action(Context::Tools, Action::OpenGit)
    );
}

#[test]
fn the_leader_lists_what_it_opens() {
    let keymap = Keymap::default();
    let contexts = [Context::Agents, Context::Global];
    let next = keymap.continuations(&contexts, &[KeyCombo::char(' ')]);
    let entries: Vec<(String, &str)> = next
        .iter()
        .map(|entry| (entry.key.short(), entry.label))
        .collect();
    for expected in [
        ("n", "new agent"),
        ("N", "new mission"),
        ("t", "theme…"),
        ("w", "wall"),
        ("k", "all keys"),
    ] {
        assert!(
            entries.contains(&(expected.0.to_owned(), expected.1)),
            "{expected:?} in {entries:?}"
        );
    }
    let next = keymap.continuations(&contexts, &[]);
    let g = next
        .iter()
        .find(|entry| entry.key == KeyCombo::char('g'))
        .unwrap();
    assert!(g.group);
    assert_eq!(g.label, "go to");
}

#[test]
fn overrides_move_every_binding_of_an_action() {
    let keymap = Keymap::with_overrides([
        KeyOverride::new("down", ["n"]),
        KeyOverride::new("next-match", ["ctrl-n"]),
        KeyOverride::new("take-over", ["ctrl-a"]),
    ])
    .unwrap();
    let contexts = [Context::Hierarchy, Context::Global];
    assert_eq!(
        keymap.action(&contexts, KeyCombo::char('n')),
        Some(Action::Down)
    );
    assert_eq!(keymap.action(&contexts, KeyCombo::char('j')), None);
    assert_eq!(
        keymap.action(&contexts, KeyCombo::ctrl('a')),
        Some(Action::TakeOver)
    );
    assert_eq!(keymap.action(&contexts, KeyCombo::char('a')), None);
}

#[test]
fn overrides_reach_one_context_or_unbind() {
    let keymap = Keymap::with_overrides([
        KeyOverride::new("first", ["home"]).in_context("agents"),
        KeyOverride::new("adopt", Vec::<String>::new()),
        KeyOverride::new("new-agent", ["c", "space n"]),
        KeyOverride::new("leave", ["ctrl-g"]),
    ])
    .unwrap();
    let agents = [Context::Agents, Context::Global];
    let g = KeyCombo::char('g');
    // gg is gone from the list only; the timeline keeps it.
    assert_eq!(keymap.resolve(&agents, &[g], g), Resolved::Unbound);
    let timeline = [Context::Timeline, Context::Global];
    assert_eq!(
        keymap.resolve(&timeline, &[g], g),
        Resolved::Action(Context::Timeline, Action::First)
    );
    assert_eq!(keymap.action(&agents, KeyCombo::char('A')), None);
    assert_eq!(
        keymap.action(&agents, KeyCombo::char('c')),
        Some(Action::NewAgent)
    );
    assert_eq!(keymap.leave_key(), KeyCombo::ctrl('g'));
}

#[test]
fn overrides_are_checked() {
    assert_eq!(
        Keymap::with_overrides([KeyOverride::new("teleport", ["t"])]).unwrap_err(),
        KeymapError::UnknownAction("teleport".into())
    );
    assert_eq!(
        Keymap::with_overrides([KeyOverride::new("down", ["n"]).in_context("kitchen")])
            .unwrap_err(),
        KeymapError::UnknownContext("kitchen".into())
    );
    assert_eq!(
        Keymap::with_overrides([KeyOverride::new("send", ["x"]).in_context("agents")]).unwrap_err(),
        KeymapError::NotInContext {
            action: "send".into(),
            context: "agents".into()
        }
    );
    assert!(matches!(
        Keymap::with_overrides([KeyOverride::new("quit", ["ctrl-"])]).unwrap_err(),
        KeymapError::InvalidKey { .. }
    ));
    assert!(matches!(
        Keymap::with_overrides([KeyOverride::new("leave", ["q"])]).unwrap_err(),
        KeymapError::LeaveKey(_)
    ));
    let error = Keymap::with_overrides([KeyOverride::new("filter", ["a"])]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "`a` is bound to both `filter` and `take-over` (Everywhere)"
    );
    // `g` alone would hide `g g` in the agents list.
    let error = Keymap::with_overrides([KeyOverride::new("diff", ["g"])]).unwrap_err();
    assert!(matches!(error, KeymapError::Ambiguous { .. }), "{error}");
    // The space bar alone would hide the leader.
    let error = Keymap::with_overrides([KeyOverride::new("diff", ["space"])]).unwrap_err();
    assert!(matches!(error, KeymapError::Ambiguous { .. }), "{error}");
}

#[test]
fn hints_follow_the_focused_panel() {
    let keymap = Keymap::default();
    let line = |contexts: &[Context]| -> String {
        keymap
            .hints(contexts)
            .iter()
            .map(|hint| format!("{} {}", hint.keys, hint.text))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    assert_eq!(
        line(&[Context::Hierarchy, Context::Global]),
        "h/j/k/l move · enter subtree · +/_ zoom · ' jump · / filter · m message · d diff · \
         a take over · z full screen · i messages · ␣ more · ? help · q quit"
    );
    assert_eq!(line(&[Context::Help]), "j/k scroll · esc close");
    assert_eq!(line(&[Context::Filter]), "enter apply · esc cancel");
    assert_eq!(
        line(&[Context::Composer]),
        "enter send · alt-enter new line · tab/shift-tab kind · esc close"
    );
    assert_eq!(
        line(&[Context::Diff]),
        "j/k scroll · f summary/patch · r refresh · o open with · esc close"
    );
    assert_eq!(line(&[Context::Confirm]), "y yes · n no");
    assert_eq!(
        line(&[Context::Screen]),
        "k/j scroll · r refresh · a take over · z back"
    );
}

#[test]
fn the_markdown_tables_list_every_binding() {
    let markdown = Keymap::default().markdown();
    assert!(markdown.starts_with(
        "### Everywhere (`global`)\n\n| Keys | What it does | Action |\n| --- | --- | --- |\n"
    ));
    assert!(markdown.contains(
        "| `q`, `ctrl-c` | quit volee (asks first when a draft would be lost) | `quit` |"
    ));
    assert!(markdown.contains("| `alt-enter`, `ctrl-j` | start a new line | `new-line` |"));
    assert!(markdown.contains("| `gg`, `home` | first agent | `first` |"));
    assert!(markdown.contains("| `space n` | launch an agent in a new worktree | `new-agent` |"));
    assert!(markdown.contains("### Hierarchy (`hierarchy`)"));
    assert_eq!(
        markdown.matches("\n| `").count(),
        Keymap::default().bindings().len()
    );
}
