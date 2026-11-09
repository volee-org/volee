//! The run loop and the terminal (ADR 0009).
//!
//! The loop waits on three things at once: the terminal, the source and an
//! animation clock. The clock ticks at about 24 frames per second only while
//! something moves (a spinner), and once a second otherwise, for elapsed
//! times. Bursts of events are folded into one update, and the screen is
//! drawn only when the model changed.

use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, Stdout, stdout};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Once};
use std::time::Duration;

use futures_util::{FutureExt, StreamExt};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event as TerminalEvent, EventStream, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::style::{Attribute, Print, SetAttribute};
use ratatui::crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
use tokio::sync::mpsc;
use tokio::time::Instant;
use volee_core::Event;

use crate::app::{Effect, FocusFailure, Model, Msg, update};
use crate::clock::Clock;
use crate::source::{Source, SourceError, TakeOver};
use crate::view::view;

/// Time between two frames while something moves.
const FRAME: Duration = Duration::from_millis(42);

/// Time between two ticks when nothing moves, for elapsed times.
const IDLE_TICK: Duration = Duration::from_secs(1);

/// The most terminal events folded into one frame.
const BURST: usize = 64;

/// What the terminal sends the loop.
#[derive(Debug)]
pub(crate) enum Input {
    /// A key, the mouse, a resize or a paste.
    Event(TerminalEvent),
    /// The process was asked to stop (`SIGINT`, `SIGTERM`, `SIGHUP`).
    Interrupted,
    /// The terminal is gone.
    Closed,
}

/// The terminal side of the loop. The real one drives a terminal; tests use
/// one backed by ratatui's test backend.
pub(crate) trait Screen {
    /// The size in cells.
    fn size(&self) -> io::Result<(u16, u16)>;

    /// Draws the model.
    fn draw(&mut self, model: &Model) -> io::Result<()>;

    /// The next input. Must be cancel safe.
    async fn input(&mut self) -> Input;

    /// Suspends the interface, runs the take over in this terminal, and
    /// comes back to the same state.
    async fn take_over(&mut self, take_over: &TakeOver) -> io::Result<ExitStatus>;

    /// Starts a program with a window of its own, detached from this
    /// terminal, and does not wait for it.
    fn detach(&mut self, program: &TakeOver) -> io::Result<()>;

    /// Runs the steps that bring a multiplexer pane forward, one after the
    /// other, keeping the terminal: they print nothing on it. The first that
    /// fails says why, and how many ran fine before it.
    fn bring_forward(&mut self, steps: &[Vec<String>]) -> Result<(), FocusFailure>;
}

/// Runs the loop until the person quits or the terminal goes away.
pub(crate) async fn event_loop<S: Screen, Src: Source>(
    screen: &mut S,
    source: Arc<Src>,
    mut updates: mpsc::Receiver<Result<Event, SourceError>>,
    model: &mut Model,
    clock: &dyn Clock,
    first: Msg,
) -> io::Result<()> {
    let (results, mut answers) = mpsc::unbounded_channel::<Msg>();
    let mut queue = VecDeque::from([first]);
    let mut source_open = true;
    let mut last_tick = Instant::now();
    loop {
        while let Some(msg) = queue.pop_front() {
            for effect in update(model, msg) {
                match effect {
                    Effect::Quit => return Ok(()),
                    Effect::TakeOver { agent, take_over } => {
                        let outcome = screen.take_over(&take_over).await;
                        let result = match outcome {
                            Ok(status) if status.success() => Ok(()),
                            Ok(status) => Err(format!("it ended with {status}")),
                            Err(error) => Err(error.to_string()),
                        };
                        // The terminal may have been resized meanwhile. Both
                        // messages come before any input typed meanwhile.
                        let (width, height) = screen.size()?;
                        queue.push_front(Msg::Resumed { agent, result });
                        queue.push_front(Msg::Resize { width, height });
                    }
                    Effect::FocusPane { agent, focus } => {
                        let result = screen.bring_forward(&focus.steps);
                        queue.push_front(Msg::PaneFocused {
                            agent,
                            focus,
                            result,
                        });
                    }
                    Effect::RunTool {
                        label,
                        detached,
                        take_over,
                    } => {
                        let result = run_tool(screen, &take_over, detached).await;
                        let (width, height) = screen.size()?;
                        queue.push_front(Msg::ToolFinished { label, result });
                        queue.push_front(Msg::Resize { width, height });
                    }
                    request => run_request(request, &source, &results),
                }
            }
        }
        if model.dirty {
            screen.draw(model)?;
            model.dirty = false;
        }

        let interval = if model.is_animating() {
            FRAME
        } else {
            IDLE_TICK
        };
        let deadline = last_tick + interval;
        tokio::select! {
            input = screen.input() => {
                match input {
                    Input::Event(event) => queue.extend(to_msg(event)),
                    Input::Interrupted | Input::Closed => return Ok(()),
                }
                // Fold a burst of input (a held key, a paste) into one frame.
                for _ in 0..BURST {
                    match screen.input().now_or_never() {
                        Some(Input::Event(event)) => queue.extend(to_msg(event)),
                        Some(Input::Interrupted | Input::Closed) => return Ok(()),
                        None => break,
                    }
                }
            }
            item = updates.recv(), if source_open => {
                // Fold what is already waiting into the same update.
                let mut events = Vec::new();
                let mut next = item;
                let ended = loop {
                    match next {
                        Some(Ok(event)) => events.push(event),
                        Some(Err(error)) => break Some(Some(error.to_string())),
                        None => break Some(None),
                    }
                    match updates.try_recv() {
                        Ok(item) => next = Some(item),
                        // Empty, or closed: the next receive tells.
                        Err(_) => break None,
                    }
                };
                if !events.is_empty() {
                    queue.push_back(Msg::Events(events));
                }
                if let Some(reason) = ended {
                    source_open = false;
                    queue.push_back(Msg::SourceClosed(reason));
                }
            }
            Some(answer) = answers.recv() => queue.push_back(answer),
            () = tokio::time::sleep_until(deadline) => {
                last_tick = Instant::now();
                queue.push_back(Msg::Tick(clock.now()));
            }
        }
    }
}

/// Runs a request to the source in a task; its answer comes back as a
/// message, so a slow source never blocks drawing.
fn run_request<Src: Source>(
    effect: Effect,
    source: &Arc<Src>,
    results: &mpsc::UnboundedSender<Msg>,
) {
    let source = Arc::clone(source);
    match effect {
        Effect::LoadPreview(agent) => spawn(results, async move {
            let result = source.preview(agent).await;
            Msg::PreviewLoaded { agent, result }
        }),
        Effect::LoadDiff { agent, full } => spawn(results, async move {
            let result = source.diff(agent, full).await;
            Msg::DiffLoaded {
                agent,
                full,
                result,
            }
        }),
        Effect::PrepareTakeOver(agent) => spawn(results, async move {
            let result = source.attach(agent).await;
            Msg::TakeOverReady { agent, result }
        }),
        Effect::Send(message) => spawn(results, async move {
            let to = message.to;
            let result = source.send_message(message).await;
            Msg::MessageSent { to, result }
        }),
        Effect::MarkRead(message) => spawn(results, async move {
            Msg::MarkedRead(source.mark_read(message).await)
        }),
        Effect::Summarize(thread) => spawn(results, async move {
            let result = source.summarize(thread).await;
            Msg::Summarized { thread, result }
        }),
        Effect::Spawn(request) => spawn(results, async move {
            Msg::Launched(source.spawn(request).await)
        }),
        Effect::CreateMission(request) => spawn(results, async move {
            Msg::MissionCreated(source.create_mission(request).await)
        }),
        Effect::Forget { agent, name } => spawn(results, async move {
            let result = source.forget(agent).await;
            Msg::Forgot { name, result }
        }),
        Effect::Adopt { agent, name } => spawn(results, async move {
            let result = source.adopt(agent).await;
            Msg::Adopted { name, result }
        }),
        // The loop runs these itself.
        Effect::Quit
        | Effect::TakeOver { .. }
        | Effect::FocusPane { .. }
        | Effect::RunTool { .. } => {}
    }
}

/// Runs a tool on a worktree: in this terminal, the interface stepping
/// aside, or detached in a window of its own. A worktree that is not on this
/// machine (an agent of another machine, or the demo's) is said plainly.
async fn run_tool<S: Screen>(
    screen: &mut S,
    take_over: &TakeOver,
    detached: bool,
) -> Result<(), String> {
    if let Some(dir) = take_over.cwd.as_deref()
        && !dir.is_dir()
    {
        return Err(format!("{} is not a folder on this machine", dir.display()));
    }
    if detached {
        return screen.detach(take_over).map_err(|error| error.to_string());
    }
    match screen.take_over(take_over).await {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("it ended with {status}")),
        Err(error) => Err(error.to_string()),
    }
}

fn spawn(results: &mpsc::UnboundedSender<Msg>, task: impl Future<Output = Msg> + Send + 'static) {
    let results = results.clone();
    tokio::spawn(async move {
        // The loop may be gone; then nobody needs the answer.
        let _ = results.send(task.await);
    });
}

/// The message of a terminal event; mouse moves are ignored.
fn to_msg(event: TerminalEvent) -> Option<Msg> {
    match event {
        TerminalEvent::Key(key) => Some(Msg::Key(key)),
        TerminalEvent::Mouse(mouse) if mouse.kind != MouseEventKind::Moved => {
            Some(Msg::Mouse(mouse))
        }
        TerminalEvent::Resize(width, height) => Some(Msg::Resize { width, height }),
        TerminalEvent::Paste(text) => Some(Msg::Paste(text)),
        _ => None,
    }
}

/// The real terminal: raw mode, alternate screen, mouse and paste.
pub(crate) struct TerminalScreen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    /// The input reader; absent while a child owns the terminal.
    events: Option<EventStream>,
    signals: Signals,
    active: bool,
}

impl std::fmt::Debug for TerminalScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalScreen")
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl TerminalScreen {
    /// Takes over the terminal. A panic hook restores it before the panic
    /// message is printed.
    pub(crate) fn enter() -> io::Result<Self> {
        let terminal = ratatui::try_init()?;
        install_panic_hook();
        execute!(stdout(), EnableMouseCapture, EnableBracketedPaste)?;
        Ok(Self {
            terminal,
            events: Some(EventStream::new()),
            signals: Signals::new()?,
            active: true,
        })
    }

    /// Gives the terminal back as it was.
    pub(crate) fn leave(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        execute!(stdout(), DisableBracketedPaste, DisableMouseCapture)?;
        ratatui::try_restore()?;
        self.terminal.show_cursor()
    }

    fn resume(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        execute!(
            stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
        self.active = true;
        // The child drew on the screen: draw everything again.
        self.terminal.clear()
    }
}

impl Drop for TerminalScreen {
    fn drop(&mut self) {
        // Nothing more can be done about a failure while dropping.
        let _ = self.leave();
    }
}

impl Screen for TerminalScreen {
    fn size(&self) -> io::Result<(u16, u16)> {
        let size = self.terminal.size()?;
        Ok((size.width, size.height))
    }

    fn draw(&mut self, model: &Model) -> io::Result<()> {
        self.terminal.draw(|frame| view(model, frame)).map(|_| ())
    }

    async fn input(&mut self) -> Input {
        let Some(events) = self.events.as_mut() else {
            return std::future::pending().await;
        };
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(event)) => Input::Event(event),
                Some(Err(error)) => {
                    tracing::warn!(%error, "cannot read the terminal");
                    Input::Closed
                }
                None => Input::Closed,
            },
            () = self.signals.recv() => Input::Interrupted,
        }
    }

    fn detach(&mut self, program: &TakeOver) -> io::Result<()> {
        let mut command = Command::new(&program.program);
        command
            .args(&program.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(cwd) = &program.cwd {
            command.current_dir(cwd);
        }
        // Its own process group: Ctrl+C in volee does not reach it.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        // Reap it when it exits, so it does not linger as a zombie.
        std::thread::spawn(move || child.wait());
        Ok(())
    }

    fn bring_forward(&mut self, steps: &[Vec<String>]) -> Result<(), FocusFailure> {
        for (done, step) in steps.iter().enumerate() {
            let Some((program, args)) = step.split_first() else {
                continue;
            };
            let failure = |reason: String| FocusFailure {
                done,
                program: program.clone(),
                reason,
            };
            let output = Command::new(program)
                .args(args)
                .stdin(Stdio::null())
                .output()
                .map_err(|error| failure(format!("cannot run it: {error}")))?;
            if !output.status.success() {
                // The multiplexer's own words, from the first line it wrote.
                let said = String::from_utf8_lossy(&output.stderr);
                let said = said.lines().next().unwrap_or_default().trim();
                return Err(failure(if said.is_empty() {
                    format!("it ended with {}", output.status)
                } else {
                    said.to_owned()
                }));
            }
        }
        Ok(())
    }

    async fn take_over(&mut self, take_over: &TakeOver) -> io::Result<ExitStatus> {
        let mut command = Command::new(&take_over.program);
        command.args(&take_over.args);
        if let Some(cwd) = &take_over.cwd {
            command.current_dir(cwd);
        }
        for name in &take_over.env_remove {
            command.env_remove(name);
        }
        with_suspended(self, command, take_over.hint.as_deref()).await
    }
}

/// Suspends the interface around a child that owns the terminal: stops
/// reading input, leaves the alternate screen, raw mode, mouse capture and
/// bracketed paste, prints the hint, runs the child with this terminal's
/// input and output while Ctrl+C is ignored by volee, then restores
/// everything, reads input again and redraws from scratch. The model is
/// untouched, so the same agent is still selected.
async fn with_suspended(
    screen: &mut TerminalScreen,
    mut command: Command,
    hint: Option<&str>,
) -> io::Result<ExitStatus> {
    // Dropping the reader stops its thread: the child owns the input now.
    screen.events = None;
    screen.leave()?;
    if let Some(hint) = hint {
        execute!(
            stdout(),
            Print("\n"),
            SetAttribute(Attribute::Dim),
            Print(hint),
            SetAttribute(Attribute::Reset),
            Print("\n\n"),
        )?;
    }
    // Ctrl+C in the child also reaches volee, whose registered signal
    // handlers keep it alive; those signals are discarded below.
    let status = tokio::task::spawn_blocking(move || command.status())
        .await
        .map_err(io::Error::other)
        .and_then(|status| status);
    screen.resume()?;
    screen.events = Some(EventStream::new());
    // Signals received while suspended were meant for the child.
    screen.signals = Signals::new()?;
    status
}

/// Restores mouse and paste modes on panic; ratatui's own hook then leaves
/// raw mode and the alternate screen.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(stdout(), DisableBracketedPaste, DisableMouseCapture);
            previous(info);
        }));
    });
}

/// The signals that ask volee to stop. Registering them also keeps the
/// default action (killing volee without restoring the terminal) away.
#[derive(Debug)]
struct Signals {
    #[cfg(unix)]
    streams: [tokio::signal::unix::Signal; 3],
}

impl Signals {
    fn new() -> io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                streams: [
                    signal(SignalKind::interrupt())?,
                    signal(SignalKind::terminate())?,
                    signal(SignalKind::hangup())?,
                ],
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    async fn recv(&mut self) {
        #[cfg(unix)]
        {
            let [interrupt, terminate, hangup] = &mut self.streams;
            tokio::select! {
                _ = interrupt.recv() => {}
                _ = terminate.recv() => {}
                _ = hangup.recv() => {}
            }
        }
        #[cfg(not(unix))]
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use volee_core::message::{MessageKind, Party};
    use volee_core::time::Timestamp;

    use super::*;
    use crate::clock::ManualClock;
    use crate::demo::DemoSource;
    use crate::keymap::Keymap;
    use crate::source::Subscription;
    use crate::theme::Theme;

    /// A screen fed by a channel and drawn into a test backend.
    struct FakeScreen {
        terminal: Terminal<TestBackend>,
        inputs: mpsc::UnboundedReceiver<Input>,
        frames: usize,
        taken_over: Vec<String>,
    }

    impl Screen for FakeScreen {
        fn size(&self) -> io::Result<(u16, u16)> {
            let Ok(size) = self.terminal.size();
            Ok((size.width, size.height))
        }

        fn draw(&mut self, model: &Model) -> io::Result<()> {
            self.frames += 1;
            let Ok(_) = self.terminal.draw(|frame| view(model, frame));
            Ok(())
        }

        async fn input(&mut self) -> Input {
            self.inputs.recv().await.unwrap_or(Input::Closed)
        }

        fn take_over(
            &mut self,
            take_over: &TakeOver,
        ) -> impl Future<Output = io::Result<ExitStatus>> {
            use std::os::unix::process::ExitStatusExt;
            self.taken_over.push(take_over.program.clone());
            std::future::ready(Ok(ExitStatus::from_raw(0)))
        }

        fn detach(&mut self, program: &TakeOver) -> io::Result<()> {
            self.taken_over.push(format!("{} &", program.program));
            Ok(())
        }

        fn bring_forward(&mut self, steps: &[Vec<String>]) -> Result<(), FocusFailure> {
            for step in steps {
                self.taken_over.push(step.join(" "));
            }
            Ok(())
        }
    }

    fn key(c: char) -> Input {
        Input::Event(TerminalEvent::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )))
    }

    fn enter() -> Input {
        Input::Event(TerminalEvent::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )))
    }

    fn screen(width: u16, height: u16) -> (FakeScreen, mpsc::UnboundedSender<Input>) {
        let (send, inputs) = mpsc::unbounded_channel();
        let screen = FakeScreen {
            terminal: Terminal::new(TestBackend::new(width, height)).unwrap(),
            inputs,
            frames: 0,
            taken_over: Vec::new(),
        };
        (screen, send)
    }

    /// Sends inputs, one batch per second, so the answers of the source
    /// arrive in between.
    fn type_later(send: mpsc::UnboundedSender<Input>, batches: Vec<Vec<Input>>) {
        tokio::spawn(async move {
            for batch in batches {
                tokio::time::sleep(Duration::from_secs(1)).await;
                for input in batch {
                    send.send(input).unwrap();
                }
            }
        });
    }

    #[tokio::test(start_paused = true)]
    async fn the_loop_draws_reacts_and_quits() {
        let start = Timestamp::from_millis(1_793_986_617_000);
        let clock = ManualClock::new(start);
        let source = DemoSource::new(Arc::new(clock.clone()));
        let Subscription {
            snapshot, updates, ..
        } = source.subscribe().await.unwrap();
        let (mut screen, send) = screen(120, 40);
        let mut model = Model::new(Theme::default(), Keymap::default(), None, (120, 40), start);
        type_later(
            send,
            vec![vec![key('j'), key('a')], vec![key('?'), key('?'), key('q')]],
        );
        event_loop(
            &mut screen,
            Arc::new(source),
            updates,
            &mut model,
            &clock,
            Msg::Events(snapshot),
        )
        .await
        .unwrap();
        assert!(screen.frames >= 1);
        assert_eq!(screen.taken_over, ["sh"]);
        assert_eq!(model.registry.agents().count(), 12);
        assert!(
            model
                .notice
                .as_ref()
                .is_some_and(|n| n.text.starts_with("Back from"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_pane_is_brought_forward_without_giving_up_the_terminal() {
        let start = Timestamp::from_millis(1_793_986_617_000);
        let clock = ManualClock::new(start);
        let source = DemoSource::new(Arc::new(clock.clone()));
        let Subscription { updates, .. } = source.subscribe().await.unwrap();
        let (mut screen, send) = screen(120, 40);
        let mut model = Model::new(Theme::default(), Keymap::default(), None, (120, 40), start);
        let step = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
        let take_over = TakeOver {
            program: String::new(),
            args: Vec::new(),
            cwd: None,
            env_remove: Vec::new(),
            hint: None,
            focus: Some(crate::source::PaneFocus {
                multiplexer: "tuios".into(),
                session: "work".into(),
                steps: vec![
                    step(&["tuios", "focus-window", "-s", "work", "3f2a"]),
                    step(&["tuios", "switch-session", "work"]),
                ],
                attach: step(&["tuios", "attach", "work"]),
            }),
        };
        type_later(send, vec![vec![key('q')]]);
        let agent = crate::demo::agent_id(start, crate::demo::INFRA);
        event_loop(
            &mut screen,
            Arc::new(source),
            updates,
            &mut model,
            &clock,
            Msg::TakeOverReady {
                agent,
                result: Ok(take_over),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            screen.taken_over,
            [
                "tuios focus-window -s work 3f2a",
                "tuios switch-session work"
            ]
        );
        let notice = model.notice.as_ref().unwrap();
        assert!(
            notice
                .text
                .ends_with("is in front: tuios pane, session work.")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_loop_sends_messages_through_the_source() {
        let start = Timestamp::from_millis(1_793_986_617_000);
        let clock = ManualClock::new(start);
        let source = DemoSource::new(Arc::new(clock.clone()));
        let Subscription {
            snapshot, updates, ..
        } = source.subscribe().await.unwrap();
        let (mut screen, send) = screen(120, 40);
        let mut model = Model::new(Theme::default(), Keymap::default(), None, (120, 40), start);
        let tools = crate::demo::agent_id(start, crate::demo::INVOICE_TOOLS);
        let answer = [
            key('\''),
            key('v'),
            key('m'),
            key('Y'),
            key('e'),
            key('s'),
            enter(),
        ];
        type_later(send, vec![answer.into(), vec![key('q')]]);
        event_loop(
            &mut screen,
            Arc::new(source),
            updates,
            &mut model,
            &clock,
            Msg::Events(snapshot),
        )
        .await
        .unwrap();
        assert_eq!(model.draft, None, "sent");
        // The answer went in the question's thread, and the question is no
        // longer waiting.
        let answer = model
            .registry
            .messages_from(Party::Human)
            .into_iter()
            .next_back()
            .unwrap();
        assert_eq!(answer.body, "Yes");
        assert_eq!(answer.kind, MessageKind::Answer);
        let question = model.registry.message(answer.in_reply_to.unwrap()).unwrap();
        assert_eq!(question.from, Party::Agent(tools));
        assert_eq!(answer.thread, question.thread);
        assert!(
            !model
                .registry
                .unanswered_questions()
                .iter()
                .any(|waiting| waiting.id == question.id),
            "the question is answered"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_loop_applies_source_events_and_stops_with_the_terminal() {
        let start = Timestamp::from_millis(1_793_986_617_000);
        let clock = ManualClock::new(start);
        let source = DemoSource::new(Arc::new(clock.clone()));
        let Subscription {
            snapshot, updates, ..
        } = source.subscribe().await.unwrap();
        let (mut screen, send) = screen(80, 24);
        let mut model = Model::new(Theme::default(), Keymap::default(), None, (80, 24), start);
        let tower = crate::demo::agent_id(start, crate::demo::TOWER);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            send.send(Input::Closed).unwrap();
        });
        event_loop(
            &mut screen,
            Arc::new(source),
            updates,
            &mut model,
            &clock,
            Msg::Events(snapshot),
        )
        .await
        .unwrap();
        // The beat at four seconds changed the lead's activity.
        let activity = model.registry.agent(tower).unwrap().last_activity.as_ref();
        assert_eq!(
            activity.map(|activity| activity.summary.as_str()),
            Some("Read missions/latency-audit/goal.md")
        );
        assert!(screen.frames >= 2, "{}", screen.frames);
    }

    #[tokio::test(start_paused = true)]
    async fn a_source_that_stops_says_why() {
        let start = Timestamp::from_millis(1_793_986_617_000);
        let clock = ManualClock::new(start);
        let source = DemoSource::new(Arc::new(clock.clone()));
        let (sender, updates) = mpsc::channel(4);
        let (mut screen, send) = screen(80, 24);
        let mut model = Model::new(Theme::default(), Keymap::default(), None, (80, 24), start);
        tokio::spawn(async move {
            sender
                .send(Err(SourceError::Disconnected("lost the daemon".into())))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
            send.send(Input::Closed).unwrap();
        });
        event_loop(
            &mut screen,
            Arc::new(source),
            updates,
            &mut model,
            &clock,
            Msg::Events(Vec::new()),
        )
        .await
        .unwrap();
        assert_eq!(model.offline.as_deref(), Some("lost the daemon"));
    }
}
