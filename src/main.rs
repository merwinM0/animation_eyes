use std::{
    f64::consts::PI,
    io::{self, Write, stdout},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::Local;

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyModifiers},
    execute, queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{
        self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;

        execute!(stdout(), EnterAlternateScreen, Hide, Clear(ClearType::All))?;

        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();

        let _ = execute!(stdout(), ResetColor, Show, LeaveAlternateScreen);
    }
}

const MOVE_WINDOW_SECONDS: f64 = 25.0;
const MOVE_DURATION_SECONDS: f64 = 1.0;

struct AnimationState {
    rng: SimpleRng,
    scheduled_move_window: Option<u64>,
    move_events: Vec<MoveEvent>,
    eye_action: Option<EyeAction>,
    next_eye_action_at: f64,
}

struct MoveEvent {
    start: f64,
    direction: f64,
}

#[derive(Clone, Copy)]
struct EyeAction {
    kind: EyeActionKind,
    start: f64,
    duration: f64,
}

#[derive(Clone, Copy)]
enum EyeActionKind {
    Blink,
    Squint,
    WinkLeft,
    WinkRight,
    Surprise,
    LookUp,
    LookDown,
}

struct EyeMotion {
    left_scale: f64,
    right_scale: f64,
    y_offset: i32,
}

impl AnimationState {
    fn new() -> Self {
        Self {
            rng: SimpleRng::new(),
            scheduled_move_window: None,
            move_events: Vec::new(),
            eye_action: None,
            next_eye_action_at: 1.0,
        }
    }

    fn move_x(&mut self, t: f64, width: f64) -> i32 {
        self.schedule_move_events(t);

        self.move_events
            .retain(|event| t <= event.start + MOVE_DURATION_SECONDS);

        let amplitude = (width / 30.0).max(1.0);
        let offset: f64 = self
            .move_events
            .iter()
            .filter_map(|event| {
                if t < event.start || t > event.start + MOVE_DURATION_SECONDS {
                    return None;
                }

                let progress = (t - event.start) / MOVE_DURATION_SECONDS;
                Some(event.direction * (progress * PI).sin() * amplitude)
            })
            .sum();

        offset.round() as i32
    }

    fn eye_motion(&mut self, t: f64, height: f64) -> EyeMotion {
        self.schedule_eye_action(t);

        let Some(action) = self.eye_action else {
            return EyeMotion {
                left_scale: 1.0,
                right_scale: 1.0,
                y_offset: 0,
            };
        };

        let progress = ((t - action.start) / action.duration).clamp(0.0, 1.0);
        let pulse = (progress * PI).sin();
        let vertical_step = (height / 30.0).round().clamp(1.0, 3.0) as i32;

        match action.kind {
            EyeActionKind::Blink => {
                let scale = if progress < 0.5 {
                    1.0 - progress * 2.0
                } else {
                    (progress - 0.5) * 2.0
                };

                EyeMotion {
                    left_scale: scale,
                    right_scale: scale,
                    y_offset: 0,
                }
            }

            EyeActionKind::Squint => EyeMotion {
                left_scale: 1.0 - pulse * 0.35,
                right_scale: 1.0 - pulse * 0.35,
                y_offset: 0,
            },

            EyeActionKind::WinkLeft => EyeMotion {
                left_scale: if progress < 0.5 {
                    1.0 - progress * 2.0
                } else {
                    (progress - 0.5) * 2.0
                },
                right_scale: 1.0,
                y_offset: 0,
            },

            EyeActionKind::WinkRight => EyeMotion {
                left_scale: 1.0,
                right_scale: if progress < 0.5 {
                    1.0 - progress * 2.0
                } else {
                    (progress - 0.5) * 2.0
                },
                y_offset: 0,
            },

            EyeActionKind::Surprise => EyeMotion {
                left_scale: 1.0 + pulse * 0.28,
                right_scale: 1.0 + pulse * 0.28,
                y_offset: 0,
            },

            EyeActionKind::LookUp => EyeMotion {
                left_scale: 1.0,
                right_scale: 1.0,
                y_offset: -(pulse * vertical_step as f64).round() as i32,
            },

            EyeActionKind::LookDown => EyeMotion {
                left_scale: 1.0,
                right_scale: 1.0,
                y_offset: (pulse * vertical_step as f64).round() as i32,
            },
        }
    }

    fn schedule_eye_action(&mut self, t: f64) {
        if let Some(action) = self.eye_action {
            if t <= action.start + action.duration {
                return;
            }

            self.eye_action = None;
            self.next_eye_action_at = t + self.rng.next_range(0.8, 2.8);
        }

        if t < self.next_eye_action_at {
            return;
        }

        let kind = self.random_eye_action_kind();
        let duration = match kind {
            EyeActionKind::Blink => 0.18,
            EyeActionKind::Squint => 0.7,
            EyeActionKind::WinkLeft | EyeActionKind::WinkRight => 0.28,
            EyeActionKind::Surprise => 0.45,
            EyeActionKind::LookUp | EyeActionKind::LookDown => 0.8,
        };

        self.eye_action = Some(EyeAction {
            kind,
            start: t,
            duration,
        });
    }

    fn random_eye_action_kind(&mut self) -> EyeActionKind {
        let roll = self.rng.next_f64();

        if roll < 0.35 {
            EyeActionKind::Squint
        } else if roll < 0.55 {
            EyeActionKind::Blink
        } else if roll < 0.70 {
            EyeActionKind::WinkLeft
        } else if roll < 0.85 {
            EyeActionKind::WinkRight
        } else if roll < 0.93 {
            EyeActionKind::Surprise
        } else if roll < 0.965 {
            EyeActionKind::LookUp
        } else {
            EyeActionKind::LookDown
        }
    }

    fn schedule_move_events(&mut self, t: f64) {
        let window = (t / MOVE_WINDOW_SECONDS).floor() as u64;

        if self.scheduled_move_window == Some(window) {
            return;
        }

        self.scheduled_move_window = Some(window);

        let count = if self.rng.next_bool() { 3 } else { 4 };
        let window_start = window as f64 * MOVE_WINDOW_SECONDS;

        for _ in 0..count {
            let start = window_start
                + self
                    .rng
                    .next_range(5.0, MOVE_WINDOW_SECONDS - MOVE_DURATION_SECONDS - 5.0);
            let direction = if self.rng.next_bool() { 1.0 } else { -1.0 };

            self.move_events.push(MoveEvent { start, direction });
        }

        self.move_events.sort_by(|a, b| a.start.total_cmp(&b.start));
    }
}

struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0x1234_5678_9abc_def0);

        Self { state: seed | 1 }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);

        self.state
    }

    fn next_f64(&mut self) -> f64 {
        self.next_u64() as f64 / u64::MAX as f64
    }

    fn next_range(&mut self, min: f64, max: f64) -> f64 {
        min + (max - min) * self.next_f64()
    }

    fn next_bool(&mut self) -> bool {
        self.next_u64() % 2 == 0
    }
}

fn main() -> io::Result<()> {
    let _guard = TerminalGuard::enter()?;
    let mut out = stdout();

    let start_time = Instant::now();
    let mut animation = AnimationState::new();

    loop {
        // 监听按键，同时用较短的 poll 间隔驱动动画刷新
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                let quit_by_q = key.code == KeyCode::Char('q');
                let quit_by_ctrl_c =
                    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);

                if quit_by_q || quit_by_ctrl_c {
                    break;
                }
            }
        }

        draw(&mut out, start_time.elapsed(), &mut animation)?;
    }

    Ok(())
}

fn draw(out: &mut io::Stdout, elapsed: Duration, animation: &mut AnimationState) -> io::Result<()> {
    let (width, height) = terminal::size()?;

    // 清空到终端默认背景色，不主动设置背景色
    queue!(out, Clear(ClearType::All))?;

    if width == 0 || height == 0 {
        out.flush()?;
        return Ok(());
    }

    let w = width as f64;
    let h = height as f64;

    // 从顶部开始数：高度 * 2/5 作为基准线
    let baseline_y = (h * 2.0 / 5.0).round() as i32;

    // 每只眼睛的宽度：1/6 * 宽
    let eye_width = (w / 6.0).round().max(1.0) as i32;

    // 左眼起始点：1/4 * 宽
    let left_start_x = (w / 4.0).round() as i32;

    // 右眼起始点：从右侧向内留出 1/4 * 宽
    let right_start_x = width as i32 - left_start_x - eye_width;

    // 向上和向下延伸的高度：1/12 * 宽
    let eye_radius_y = (w / 12.0).round().max(1.0) as i32;

    let t = elapsed.as_secs_f64();

    // 左右移动：按 MOVE_WINDOW_SECONDS 的窗口随机触发，每次快速偏移后回到原位
    let move_x = animation.move_x(t, w);

    // 眼睛动作由随机调度器控制，同一时间只执行一种眼睛动作。
    // 左右移动是独立动作，可以叠加在眨眼、眯眼或不动状态上。
    let eye_motion = animation.eye_motion(t, h);
    let left_radius_y = (eye_radius_y as f64 * eye_motion.left_scale)
        .round()
        .max(1.0) as i32;
    let right_radius_y = (eye_radius_y as f64 * eye_motion.right_scale)
        .round()
        .max(1.0) as i32;
    let eye_center_y = baseline_y + eye_motion.y_offset;

    // 使用 2x2 象限块字符绘制眼睛，边缘比整格像素更细腻
    queue!(out, SetForegroundColor(Color::White))?;

    draw_eye(
        out,
        left_start_x + move_x,
        eye_center_y,
        eye_width,
        left_radius_y,
        width,
        height,
    )?;

    draw_eye(
        out,
        right_start_x + move_x,
        eye_center_y,
        eye_width,
        right_radius_y,
        width,
        height,
    )?;

    draw_time(out, width, height)?;

    queue!(out, ResetColor)?;

    out.flush()?;
    Ok(())
}

fn draw_time(out: &mut io::Stdout, screen_width: u16, screen_height: u16) -> io::Result<()> {
    let time_text = Local::now().format("%H:%M").to_string();
    let time_width = time_text.chars().count() as u16;

    let x = screen_width.saturating_sub(time_width);
    let y = screen_height.saturating_sub(1);

    queue!(
        out,
        SetForegroundColor(Color::White),
        MoveTo(x, y),
        Print(time_text)
    )?;

    Ok(())
}

fn draw_eye(
    out: &mut io::Stdout,
    start_x: i32,
    center_y: i32,
    eye_width: i32,
    radius_y: i32,
    screen_width: u16,
    screen_height: u16,
) -> io::Result<()> {
    let screen_w = screen_width as i32;
    let screen_h = screen_height as i32;

    let end_x = start_x + eye_width;

    let center_x = start_x as f64 + eye_width as f64 / 2.0;
    let center_y_f = center_y as f64;

    let radius_x = (eye_width as f64 / 2.0).max(1.0);
    let radius_y_f = (radius_y as f64).max(1.0);

    let min_x = start_x.max(0);
    let max_x = end_x.min(screen_w);

    let min_y = (center_y - radius_y).max(0);
    let max_y = (center_y + radius_y).min(screen_h - 1);

    for y in min_y..=max_y {
        for x in min_x..max_x {
            // 每个终端字符格拆成 2x2 个采样点，使用 Unicode 象限块显示。
            // 这样边缘不再只能按整格绘制，看起来会更平滑。
            let mut mask = 0;
            let samples = [
                (0.25, 0.25, 1), // 左上
                (0.75, 0.25, 2), // 右上
                (0.25, 0.75, 4), // 左下
                (0.75, 0.75, 8), // 右下
            ];

            for (offset_x, offset_y, bit) in samples {
                let px = x as f64 + offset_x;
                let py = y as f64 + offset_y;

                let dx = (px - center_x) / radius_x;
                let dy = (py - center_y_f) / radius_y_f;

                if dx * dx + dy * dy <= 1.0 {
                    mask |= bit;
                }
            }

            if mask != 0 {
                queue!(out, MoveTo(x as u16, y as u16), Print(quadrant_char(mask)))?;
            }
        }
    }

    Ok(())
}

fn quadrant_char(mask: u8) -> char {
    match mask {
        1 => '▘',
        2 => '▝',
        3 => '▀',
        4 => '▖',
        5 => '▌',
        6 => '▞',
        7 => '▛',
        8 => '▗',
        9 => '▚',
        10 => '▐',
        11 => '▜',
        12 => '▄',
        13 => '▙',
        14 => '▟',
        15 => '█',
        _ => ' ',
    }
}
