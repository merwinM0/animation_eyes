mod animation;
mod render;

use std::{
    io::{self, BufWriter, IsTerminal, stdout},
    time::{Duration, Instant},
};

use chrono::Local;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    style::ResetColor,
    terminal::{
        self, DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen,
        disable_raw_mode, enable_raw_mode,
    },
};

use animation::AnimationState;
use render::{Renderer, Scene};

const FRAME_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Default)]
struct TerminalGuard {
    raw_mode: bool,
    alternate_screen: bool,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        let mut guard = Self::default();
        enable_raw_mode()?;
        guard.raw_mode = true;
        // 提前记录清理责任，初始化中途失败也会恢复终端。
        guard.alternate_screen = true;
        execute!(stdout(), EnterAlternateScreen, DisableLineWrap, Hide)?;
        Ok(guard)
    }

    fn restore(&mut self) -> io::Result<()> {
        let raw_result = if self.raw_mode {
            disable_raw_mode().inspect(|()| self.raw_mode = false)
        } else {
            Ok(())
        };
        // 即使恢复 raw mode 失败，也继续尝试恢复光标和主屏幕。
        let screen_result = if self.alternate_screen {
            execute!(
                stdout(),
                ResetColor,
                Show,
                EnableLineWrap,
                LeaveAlternateScreen
            )
            .inspect(|()| self.alternate_screen = false)
        } else {
            Ok(())
        };
        raw_result.and(screen_result)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Drop 无法返回错误；正常退出会显式 restore 并传播错误，这里仅作兜底。
        let _ = self.restore();
    }
}

struct AnimationClock {
    elapsed: Duration,
    last_update: Instant,
    paused: bool,
}

impl AnimationClock {
    fn new(now: Instant) -> Self {
        Self {
            elapsed: Duration::ZERO,
            last_update: now,
            paused: false,
        }
    }

    fn update(&mut self, now: Instant) {
        if !self.paused {
            self.elapsed += now.saturating_duration_since(self.last_update);
        }
        self.last_update = now;
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Input {
    Quit,
    Changed,
    Ignored,
}

fn handle_key(key: KeyEvent, clock: &mut AnimationClock, animation: &mut AnimationState) -> Input {
    if key.kind != KeyEventKind::Press {
        return Input::Ignored;
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('q' | 'Q') => Input::Quit,
        KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => Input::Quit,
        KeyCode::Char('p' | 'P') => {
            clock.paused = !clock.paused;
            Input::Changed
        }
        KeyCode::Char(' ') => {
            animation.next_expression(clock.elapsed.as_secs_f64());
            Input::Changed
        }
        _ => Input::Ignored,
    }
}

fn main() -> io::Result<()> {
    if !io::stdin().is_terminal() || !stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "请在交互式终端中运行本程序",
        ));
    }
    let mut guard = TerminalGuard::enter()?;
    let result = run();
    let restored = guard.restore();
    result.and(restored)
}

fn run() -> io::Result<()> {
    let mut out = BufWriter::new(stdout().lock());
    let mut renderer = Renderer::default();
    let mut animation = AnimationState::new();
    let now = Instant::now();
    let mut clock = AnimationClock::new(now);
    let mut next_frame = now;
    let mut next_clock_update = now;
    let mut time_text = String::new();
    let mut size = terminal::size()?;
    let mut dirty = true;

    loop {
        let now = Instant::now();
        clock.update(now);
        if now >= next_frame || dirty {
            if now >= next_clock_update {
                time_text = Local::now().format("%H:%M").to_string();
                next_clock_update = now + Duration::from_secs(1);
            }
            let pose = animation.sample(clock.elapsed.as_secs_f64());
            renderer.draw(
                &mut out,
                size,
                &Scene {
                    pose,
                    expression: animation.expression(),
                    paused: clock.paused,
                    time: &time_text,
                },
            )?;
            next_frame = now + FRAME_INTERVAL;
            dirty = false;
        }

        // 以帧截止时间等待输入，避免固定 poll 延时叠加绘制耗时。
        if event::poll(next_frame.saturating_duration_since(Instant::now()))? {
            match event::read()? {
                Event::Resize(width, height) => {
                    size = (width, height);
                    renderer.invalidate();
                    dirty = true;
                }
                Event::Key(key) => {
                    clock.update(Instant::now());
                    match handle_key(key, &mut clock, &mut animation) {
                        Input::Quit => break,
                        Input::Changed => dirty = true,
                        Input::Ignored => {}
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_freezes_animation_time_and_resume_does_not_jump() {
        let start = Instant::now();
        let mut clock = AnimationClock::new(start);
        clock.update(start + Duration::from_secs(1));
        clock.paused = true;
        clock.update(start + Duration::from_secs(100));
        assert_eq!(clock.elapsed, Duration::from_secs(1));
        clock.paused = false;
        clock.update(start + Duration::from_secs(101));
        assert_eq!(clock.elapsed, Duration::from_secs(2));
    }

    #[test]
    fn quit_keys_include_escape_uppercase_q_and_ctrl_c() {
        let mut clock = AnimationClock::new(Instant::now());
        let mut animation = AnimationState::new();
        for (code, modifiers) in [
            (KeyCode::Esc, KeyModifiers::NONE),
            (KeyCode::Char('q'), KeyModifiers::NONE),
            (KeyCode::Char('Q'), KeyModifiers::NONE),
            (KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            assert_eq!(
                handle_key(KeyEvent::new(code, modifiers), &mut clock, &mut animation),
                Input::Quit
            );
        }
        assert_eq!(
            handle_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
                &mut clock,
                &mut animation
            ),
            Input::Ignored
        );
    }

    #[test]
    fn repeated_or_released_keys_do_not_toggle_pause() {
        let mut clock = AnimationClock::new(Instant::now());
        let mut animation = AnimationState::new();
        for kind in [KeyEventKind::Release, KeyEventKind::Repeat] {
            let key = KeyEvent::new_with_kind(KeyCode::Char('p'), KeyModifiers::NONE, kind);
            assert_eq!(handle_key(key, &mut clock, &mut animation), Input::Ignored);
            assert!(!clock.paused);
        }
        let key = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        assert_eq!(handle_key(key, &mut clock, &mut animation), Input::Changed);
        assert!(clock.paused);
    }
}
