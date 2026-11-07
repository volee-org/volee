//! The run loop and the terminal (ADR 0009).
//!
//! The loop waits on three things at once: the terminal, the source and an
//! animation clock. The clock ticks at about 24 frames per second only while
//! something moves (a spinner), and once a second otherwise, for elapsed
//! times. Bursts of events are folded into one update, and the screen is
//! drawn only when the model changed.

use std::collections::VecDeque;
use std::io::{self, Stdout, stdout};
use std::process::{Command, ExitStatus};
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
use ratatui::crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
use tokio::sync::mpsc;
use tokio::time::Instant;
use volee_core::Event;

use crate::app::{Effect, Model, Msg, update};
use crate::clock::Clock;
use crate::source::Source;
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

    /// Suspends the interface, runs `command` in this terminal, and comes
    /// back to the same state.
    async fn take_over(&mut self, command: Command) -> io::Result<ExitStatus>;
}

/// Runs the loop until the person quits or the terminal goes away.
pub(crate) async fn event_loop<S: Screen, Src: Source>(
    screen: &mut S,
    source: Arc<Src>,
    mut updates: mpsc::Receiver<Event>,
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
                    Effect::LoadPreview(agent) => {
                        let source = Arc::clone(&source);
                        let results = results.clone();
                        tokio::spawn(async move {
                            let result = source.preview(agent).await.map_err(|e| e.to_string());
                            // The loop may be gone; then nobody needs the preview.
                            let _ = results.send(Msg::PreviewLoaded { agent, result });
                        });
                    }
                    Effect::TakeOver(agent) => {
                        let msg = match source.attach_command(agent) {
                            None => Msg::TakeOverUnavailable(agent),
                            Some(command) => {
                                let outcome = screen.take_over(command).await;
                                Msg::Resumed {
                                    agent,
                                    result: match outcome {
                                        Ok(status) if status.success() => Ok(()),
                                        Ok(status) => Err(format!("it ended with {status}")),
                                        Err(error) => Err(error.to_string()),
                                    },
                                }
                            }
                        };
                        // The terminal may have been resized meanwhile. Both
                        // messages come before any input typed after `a`.
                        let (width, height) = screen.size()?;
                        queue.push_front(msg);
                        queue.push_front(Msg::Resize { width, height });
                    }
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
            event = updates.recv(), if source_open => {
                if let Some(event) = event {
                    // Fold what is already waiting into the same update.
                    let mut events = vec![event];
                    while let Ok(event) = updates.try_recv() {
                        events.push(event);
                    }
                    queue.push_back(Msg::Events(events));
                } else {
                    source_open = false;
                    queue.push_back(Msg::SourceClosed);
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

    async fn take_over(&mut self, command: Command) -> io::Result<ExitStatus> {
        with_suspended(self, command).await
    }
}

/// Suspends the interface around a child that owns the terminal: stops
/// reading input, leaves the alternate screen, raw mode, mouse capture and
/// bracketed paste, runs the child with this terminal's input and output
/// while Ctrl+C is ignored by volee, then restores everything, reads input
/// again and redraws from scratch. The model is untouched, so the same agent
/// is still selected.
async fn with_suspended(
    screen: &mut TerminalScreen,
    mut command: Command,
) -> io::Result<ExitStatus> {
    // Dropping the reader stops its thread: the child owns the input now.
    screen.events = None;
    screen.leave()?;
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

        fn take_over(&mut self, command: Command) -> impl Future<Output = io::Result<ExitStatus>> {
            use std::os::unix::process::ExitStatusExt;
            self.taken_over
                .push(command.get_program().to_string_lossy().into_owned());
            std::future::ready(Ok(ExitStatus::from_raw(0)))
        }
    }

    fn key(c: char) -> Input {
        Input::Event(TerminalEvent::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )))
    }

    #[tokio::test(start_paused = true)]
    async fn the_loop_draws_reacts_and_quits() {
        let start = Timestamp::from_millis(1_793_986_617_000);
        let clock = ManualClock::new(start);
        let source = DemoSource::new(Arc::new(clock.clone()));
        let Subscription { snapshot, updates } = source.subscribe().await.unwrap();
        let (send, inputs) = mpsc::unbounded_channel();
        let mut screen = FakeScreen {
            terminal: Terminal::new(TestBackend::new(120, 40)).unwrap(),
            inputs,
            frames: 0,
            taken_over: Vec::new(),
        };
        let mut model = Model::new(Theme::default(), Keymap::default(), None, (120, 40), start);
        for input in [key('j'), key('a'), key('?'), key('?'), key('q')] {
            send.send(input).unwrap();
        }
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
    async fn the_loop_applies_source_events_and_stops_with_the_terminal() {
        let start = Timestamp::from_millis(1_793_986_617_000);
        let clock = ManualClock::new(start);
        let source = DemoSource::new(Arc::new(clock.clone()));
        let Subscription { snapshot, updates } = source.subscribe().await.unwrap();
        let (send, inputs) = mpsc::unbounded_channel();
        let mut screen = FakeScreen {
            terminal: Terminal::new(TestBackend::new(80, 24)).unwrap(),
            inputs,
            frames: 0,
            taken_over: Vec::new(),
        };
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
}
