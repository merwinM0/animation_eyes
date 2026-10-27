mod animation;
mod render;

use std::{
    env,
    ffi::OsString,
    io::{self, IsTerminal, stdout},
    time::{Duration, Instant},
};

use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    style::ResetColor,
    terminal::{
        self, DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen,
        disable_raw_mode, enable_raw_mode,
    },
};

use animation::{Companion, Input, Point};
use render::{Layout, Renderer, Style};

const HELP: &str = "Eyes · 当前终端里的陪伴双眼

cargo run -- [--no-mouse] [--reduced-motion] [--mono]

鼠标移动：注视；眼睛附近轻柔移动或按住左键：抚摸。
空格 / 单击：打招呼；Enter：安抚；方向键：改变注视方向。
B：点阵 / 实心块；C：淡冷暖 / 纯白；M：减少动态；P：暂停。
Q / Esc / Ctrl+C：退出，恢复终端光标、换行、鼠标与焦点报告。

--no-mouse        不接管终端鼠标，使用键盘交互
--reduced-motion  取消呼吸浮动，减缓注视运动
--mono            只用白色，也可设置 NO_COLOR
--frames N        运行 N 个动画帧后退出，用于伪终端测试
--help            显示说明

不新建图形窗口，不访问摄像头、麦克风或网络，不记录键盘文字。";

#[derive(Debug, Default)]
struct Options {
    no_mouse: bool,
    reduced_motion: bool,
    monochrome: bool,
    frames: Option<usize>,
    help: bool,
}

impl Options {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> io::Result<Self> {
        let mut options = Self::default();
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.to_str() {
                Some("--no-mouse") => options.no_mouse = true,
                Some("--reduced-motion") => options.reduced_motion = true,
                Some("--mono") => options.monochrome = true,
                Some("--help" | "-h") => options.help = true,
                Some("--frames") => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| invalid("--frames 需要正整数"))?;
                    options.frames = Some(
                        value
                            .to_str()
                            .and_then(|text| text.parse().ok())
                            .filter(|count| (1..=1_000_000).contains(count))
                            .ok_or_else(|| invalid("--frames 必须在 1～1000000 之间"))?,
                    );
                }
                _ => {
                    return Err(invalid(&format!(
                        "未知参数：{}；使用 --help 查看帮助",
                        argument.to_string_lossy()
                    )));
                }
            }
        }
        Ok(options)
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[derive(Default)]
struct TerminalGuard {
    raw: bool,
    screen: bool,
    mouse: bool,
    focus: bool,
}

impl TerminalGuard {
    fn enter(mouse: bool) -> io::Result<Self> {
        let mut guard = Self::default();
        enable_raw_mode()?;
        guard.raw = true;
        guard.screen = true;
        guard.focus = true;
        execute!(
            stdout(),
            EnterAlternateScreen,
            DisableLineWrap,
            Hide,
            EnableFocusChange
        )?;
        if mouse {
            guard.mouse = true;
            execute!(stdout(), EnableMouseCapture)?;
        }
        Ok(guard)
    }

    fn restore(&mut self) -> io::Result<()> {
        // 各部分独立恢复，某一步失败也继续尝试其他步骤。
        let mouse = if self.mouse {
            execute!(stdout(), DisableMouseCapture).inspect(|()| self.mouse = false)
        } else {
            Ok(())
        };
        let focus = if self.focus {
            execute!(stdout(), DisableFocusChange).inspect(|()| self.focus = false)
        } else {
            Ok(())
        };
        let screen = if self.screen {
            execute!(
                stdout(),
                ResetColor,
                Show,
                EnableLineWrap,
                LeaveAlternateScreen
            )
            .inspect(|()| self.screen = false)
        } else {
            Ok(())
        };
        let raw = if self.raw {
            disable_raw_mode().inspect(|()| self.raw = false)
        } else {
            Ok(())
        };
        mouse.and(focus).and(screen).and(raw)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // 正常退出显式恢复并传播错误；Drop 只负责初始化失败或展开时兜底。
        let _ = self.restore();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Effect {
    Quit,
    Repaint,
    Pending,
    Ignored,
}

struct Controls {
    size: (u16, u16),
    style: Style,
    mouse_enabled: bool,
    focused: bool,
    paused: bool,
    reduced_motion: bool,
    pointer: Option<Point>,
    pointer_for: f32,
    contact: bool,
    greeting: bool,
    soothe_for: f32,
}

impl Controls {
    fn new(size: (u16, u16), options: &Options) -> Self {
        Self {
            size,
            style: Style {
                monochrome: options.monochrome,
                ..Style::default()
            },
            mouse_enabled: !options.no_mouse,
            focused: true,
            paused: false,
            reduced_motion: options.reduced_motion,
            pointer: None,
            pointer_for: 0.0,
            contact: false,
            greeting: false,
            soothe_for: 0.0,
        }
    }

    fn interval(&self) -> Duration {
        Duration::from_nanos(
            1_000_000_000
                / if self.focused && !self.paused && !self.reduced_motion {
                    60
                } else {
                    30
                },
        )
    }

    fn handle(&mut self, event: Event) -> Effect {
        match event {
            Event::Resize(width, height) => {
                self.size = (width, height);
                self.pointer = None;
                self.contact = false;
                Effect::Repaint
            }
            Event::FocusLost => {
                self.focused = false;
                self.pointer = None;
                self.contact = false;
                self.soothe_for = 0.0;
                self.greeting = false;
                Effect::Pending
            }
            Event::FocusGained => {
                self.focused = true;
                Effect::Pending
            }
            Event::Key(key) => self.key(key),
            Event::Mouse(mouse) if self.mouse_enabled => {
                if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
                    self.contact = false;
                }
                if !self.focused
                    || self.paused
                    || mouse.column >= self.size.0
                    || mouse.row >= self.size.1
                {
                    return Effect::Ignored;
                }
                match mouse.kind {
                    MouseEventKind::Moved
                    | MouseEventKind::Drag(MouseButton::Left)
                    | MouseEventKind::Down(MouseButton::Left)
                    | MouseEventKind::Up(MouseButton::Left) => {
                        self.pointer =
                            Some(Layout::new(self.size).pointer(mouse.column, mouse.row));
                        self.pointer_for = 2.0;
                        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                            self.contact = true;
                            self.greeting = true;
                        }
                        Effect::Pending
                    }
                    _ => Effect::Ignored,
                }
            }
            _ => Effect::Ignored,
        }
    }

    fn key(&mut self, key: KeyEvent) -> Effect {
        if key.kind == KeyEventKind::Release {
            return Effect::Ignored;
        }
        let repeatable = matches!(
            key.code,
            KeyCode::Enter | KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
        );
        if key.kind == KeyEventKind::Repeat && !repeatable {
            return Effect::Ignored;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => return Effect::Quit,
            KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Effect::Quit;
            }
            _ => {}
        }
        if !self.focused {
            return Effect::Ignored;
        }
        match key.code {
            KeyCode::Char('p' | 'P') => {
                self.paused = !self.paused;
                Effect::Repaint
            }
            KeyCode::Char('b' | 'B') => {
                self.style.mode.toggle();
                Effect::Repaint
            }
            KeyCode::Char('c' | 'C') => {
                self.style.monochrome = !self.style.monochrome;
                Effect::Repaint
            }
            KeyCode::Char('m' | 'M') => {
                self.reduced_motion = !self.reduced_motion;
                Effect::Repaint
            }
            _ if self.paused => Effect::Ignored,
            KeyCode::Char(' ') => {
                self.greeting = true;
                Effect::Pending
            }
            KeyCode::Enter => {
                self.soothe_for = 2.5;
                Effect::Pending
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.pointer = Some(match key.code {
                    KeyCode::Left => Point::new(-1.2, 0.0),
                    KeyCode::Right => Point::new(1.2, 0.0),
                    KeyCode::Up => Point::new(0.0, -0.8),
                    _ => Point::new(0.0, 0.8),
                });
                self.pointer_for = 1.5;
                Effect::Pending
            }
            _ => Effect::Ignored,
        }
    }

    fn input(&mut self, elapsed: f32) -> Input {
        let elapsed = if elapsed.is_finite() {
            elapsed.max(0.0)
        } else {
            0.0
        };
        self.pointer_for = (self.pointer_for - elapsed).max(0.0);
        self.soothe_for = (self.soothe_for - elapsed).max(0.0);
        Input {
            pointer: if self.pointer_for > 0.0 || self.contact {
                self.pointer
            } else {
                None
            },
            focused: self.focused,
            contact: self.contact,
            greet: std::mem::take(&mut self.greeting),
            soothe: self.soothe_for > 0.0,
        }
    }
}

fn advance_deadline(deadline: Instant, now: Instant, interval: Duration) -> Instant {
    let next = deadline + interval;
    if next > now { next } else { now + interval }
}

fn main() -> io::Result<()> {
    let mut options = Options::parse(env::args_os().skip(1))?;
    if options.help {
        println!("{HELP}");
        return Ok(());
    }
    options.monochrome |= env::var_os("NO_COLOR").is_some();
    if !io::stdin().is_terminal() || !stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "请在交互式终端中运行本程序；不需要图形桌面",
        ));
    }
    let mut guard = TerminalGuard::enter(!options.no_mouse)?;
    let result = run(&options);
    let restored = guard.restore();
    result.and(restored)
}

fn run(options: &Options) -> io::Result<()> {
    let mut controls = Controls::new(terminal::size()?, options);
    let mut model = Companion::default();
    let mut renderer = Renderer::default();
    let mut out = stdout().lock();
    let mut last_tick = Instant::now();
    let mut deadline = last_tick;
    let mut repaint = true;
    let mut frames = 0;
    'animation: loop {
        let now = Instant::now();
        let due = now >= deadline;
        if due || repaint {
            model.set_reduced_motion(controls.reduced_motion);
            if due {
                let elapsed = now.saturating_duration_since(last_tick).as_secs_f32();
                last_tick = now;
                if !controls.paused {
                    model.update(elapsed, controls.input(elapsed));
                }
            }
            renderer.draw(&mut out, controls.size, model.pose(), controls.style)?;
            if due {
                frames += 1;
                deadline = advance_deadline(deadline, Instant::now(), controls.interval());
                if options.frames.is_some_and(|limit| frames >= limit) {
                    break;
                }
            }
            repaint = false;
        }
        if event::poll(deadline.saturating_duration_since(Instant::now()))? {
            // 合并待处理的鼠标/缩放事件，但保留批次上限，保证动画和退出响应。
            for _ in 0..64 {
                let event = event::read()?;
                if matches!(event, Event::Resize(..)) {
                    renderer.invalidate();
                }
                match controls.handle(event) {
                    Effect::Quit => break 'animation,
                    Effect::Repaint => repaint = true,
                    Effect::Pending | Effect::Ignored => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::MouseEvent;

    fn controls() -> Controls {
        Controls::new((80, 24), &Options::default())
    }
    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn keyboard_interaction_is_not_expression_cycling() {
        let mut controls = controls();
        assert_eq!(controls.handle(key(KeyCode::Char(' '))), Effect::Pending);
        assert!(controls.input(0.016).greet);
        assert!(!controls.input(0.016).greet);
        controls.handle(key(KeyCode::Enter));
        assert!(controls.input(0.016).soothe);
        assert!(!controls.input(3.0).soothe);
        controls.handle(key(KeyCode::Right));
        assert_eq!(controls.input(0.016).pointer, Some(Point::new(1.2, 0.0)));
        assert_eq!(controls.input(2.0).pointer, None);
    }

    #[test]
    fn mouse_contact_and_focus_loss_cannot_leave_a_stuck_button() {
        let mut controls = controls();
        let mouse = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: 25,
                row: 12,
                modifiers: KeyModifiers::NONE,
            })
        };
        controls.handle(mouse(MouseEventKind::Down(MouseButton::Left)));
        assert!(controls.input(0.016).contact);
        assert!(controls.input(3.0).pointer.is_some());
        controls.handle(Event::FocusLost);
        let input = controls.input(0.016);
        assert!(!input.focused && !input.contact && !input.greet);
        assert_eq!(input.pointer, None);
    }

    #[test]
    fn no_mouse_mode_ignores_mouse_but_keeps_keyboard_alternatives() {
        let mut controls = Controls::new(
            (80, 24),
            &Options {
                no_mouse: true,
                ..Options::default()
            },
        );
        assert_eq!(
            controls.handle(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: 25,
                row: 12,
                modifiers: KeyModifiers::NONE
            })),
            Effect::Ignored
        );
        assert_eq!(controls.handle(key(KeyCode::Enter)), Effect::Pending);
    }

    #[test]
    fn pause_blocks_interaction_but_not_display_changes_or_quit() {
        let mut controls = controls();
        controls.handle(key(KeyCode::Char('p')));
        assert!(controls.paused);
        assert_eq!(controls.handle(key(KeyCode::Char(' '))), Effect::Ignored);
        assert_eq!(controls.handle(key(KeyCode::Char('b'))), Effect::Repaint);
        assert_eq!(controls.handle(key(KeyCode::Esc)), Effect::Quit);
    }

    #[test]
    fn repeats_do_not_toggle_modes_and_releases_are_ignored() {
        let mut controls = controls();
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            assert_eq!(
                controls.handle(Event::Key(KeyEvent::new_with_kind(
                    KeyCode::Char('p'),
                    KeyModifiers::NONE,
                    kind
                ))),
                Effect::Ignored
            );
        }
        assert!(!controls.paused);
    }

    #[test]
    fn deadlines_do_not_accumulate_render_time_or_chase_expired_frames() {
        let start = Instant::now();
        let interval = Duration::from_nanos(1_000_000_000 / 60);
        let mut next = start;
        for _ in 0..60 {
            next = advance_deadline(next, next + Duration::from_millis(2), interval);
        }
        assert!(Duration::from_secs(1).abs_diff(next - start) < Duration::from_micros(1));
        let later = start + Duration::from_secs(10);
        assert_eq!(advance_deadline(start, later, interval), later + interval);
    }

    #[test]
    fn cli_remains_terminal_only() {
        let parse = |args: &[&str]| Options::parse(args.iter().map(OsString::from));
        assert!(parse(&["--help"]).unwrap().help);
        assert!(parse(&["--no-mouse", "--mono", "--reduced-motion"]).is_ok());
        assert_eq!(parse(&["--frames", "5"]).unwrap().frames, Some(5));
        for args in [
            &["--frames", "0"][..],
            &["--frames"],
            &["--borderless"],
            &["--preview", "file.ppm"],
        ] {
            assert!(parse(args).is_err());
        }
    }
}
