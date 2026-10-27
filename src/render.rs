use std::io::{self, Write};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType},
};

use crate::animation::{EyePose, FacePose, Point};

const MAX_CELLS: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Dots,
    Blocks,
}

impl Mode {
    pub(crate) fn toggle(&mut self) {
        *self = match self {
            Self::Dots => Self::Blocks,
            Self::Blocks => Self::Dots,
        };
    }

    fn samples(self) -> &'static [(f32, f32, u8)] {
        match self {
            Self::Dots => &[
                (0.25, 0.125, 1),
                (0.75, 0.125, 8),
                (0.25, 0.375, 2),
                (0.75, 0.375, 16),
                (0.25, 0.625, 4),
                (0.75, 0.625, 32),
                (0.25, 0.875, 64),
                (0.75, 0.875, 128),
            ],
            Self::Blocks => &[
                (0.25, 0.25, 1),
                (0.75, 0.25, 2),
                (0.25, 0.75, 4),
                (0.75, 0.75, 8),
            ],
        }
    }

    fn character(self, mask: u8) -> char {
        if mask == 0 {
            return ' ';
        }
        match self {
            // u8 掩码使码点限定在 U+2800..=U+28FF，必然是合法 Unicode 标量。
            Self::Dots => char::from_u32(0x2800 + u32::from(mask))
                .expect("all eight-dot masks are valid Unicode"),
            Self::Blocks => [
                ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
            ][usize::from(mask)],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Style {
    pub(crate) mode: Mode,
    pub(crate) monochrome: bool,
}

/// 字符格通常高约为宽的两倍；在物理比例一致的世界坐标中布局与命中测试。
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    center: Point,
    unit: f32,
}

impl Layout {
    pub(crate) fn new(size: (u16, u16)) -> Self {
        Self {
            center: Point::new(f32::from(size.0) * 0.5, f32::from(size.1) * 0.5),
            unit: (f32::from(size.0) / 2.2)
                .min(f32::from(size.1) * 2.0 / 1.45)
                .max(0.001),
        }
    }

    pub(crate) fn pointer(self, column: u16, row: u16) -> Point {
        Point::new(
            (f32::from(column) + 0.5 - self.center.x) / self.unit,
            (f32::from(row) + 0.5 - self.center.y) * 2.0 / self.unit,
        )
    }
}

#[derive(Default)]
struct Frame {
    size: (u16, u16),
    cells: Vec<char>,
}

impl Frame {
    fn clear(&mut self, size: (u16, u16)) -> io::Result<()> {
        let count = usize::from(size.0) * usize::from(size.1);
        if count > MAX_CELLS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "终端尺寸超过安全绘制范围",
            ));
        }
        if count > self.cells.len() {
            self.cells
                .try_reserve(count - self.cells.len())
                .map_err(io::Error::other)?;
        }
        self.cells.resize(count, ' ');
        self.cells.fill(' ');
        self.size = size;
        Ok(())
    }

    fn compose(&mut self, pose: FacePose, mode: Mode) {
        let (width, height) = self.size;
        if width == 0 || height == 0 {
            return;
        }
        if width < 12 || height < 4 {
            // 极小窗口也只显示眼睛，不添加嘴、状态栏或提示文字。
            let text = if width >= 3 {
                "o o"
            } else if width == 2 {
                "oo"
            } else {
                "o"
            };
            let start = usize::from(height / 2) * usize::from(width)
                + (usize::from(width) - text.len()) / 2;
            for (offset, ch) in text.chars().enumerate() {
                self.cells[start + offset] = ch;
            }
            return;
        }
        let layout = Layout::new(self.size);
        for eye in pose.eyes {
            self.eye(eye, layout, mode);
        }
    }

    fn eye(&mut self, eye: EyePose, layout: Layout, mode: Mode) {
        let center = layout.center
            + Point::new(eye.center.x * layout.unit, eye.center.y * layout.unit * 0.5);
        let radius = Point::new(eye.radius.x * layout.unit, eye.radius.y * layout.unit * 0.5);
        let extent_y = radius.y * (1.0 + eye.tilt.abs());
        let min_x = (center.x - radius.x).floor().max(0.0) as u16;
        let max_x = (center.x + radius.x).ceil().min(f32::from(self.size.0)) as u16;
        let min_y = (center.y - extent_y).floor().max(0.0) as u16;
        let max_y = (center.y + extent_y).ceil().min(f32::from(self.size.1)) as u16;
        let inverse = Point::new(radius.x.recip(), radius.y.recip());
        let samples = mode.samples();
        let mut offsets = [(0.0, 0.0, 0); 8];
        for (output, &(x, y, bit)) in offsets.iter_mut().zip(samples) {
            let dx = x * inverse.x;
            *output = (dx, y * inverse.y - eye.tilt * dx, bit);
        }
        for y in min_y..max_y {
            for x in min_x..max_x {
                let dx = (f32::from(x) - center.x) * inverse.x;
                let dy = (f32::from(y) - center.y) * inverse.y - eye.tilt * dx;
                let mut mask = 0;
                for &(ox, oy, bit) in &offsets[..samples.len()] {
                    if contains(dx + ox, dy + oy, eye) {
                        mask |= bit;
                    }
                }
                if mask != 0 {
                    self.cells[usize::from(y) * usize::from(self.size.0) + usize::from(x)] =
                        mode.character(mask);
                }
            }
        }
    }
}

fn contains(x: f32, y: f32, eye: EyePose) -> bool {
    let squared = x * x + y * y;
    let rounded = squared + eye.roundness * (x.powi(4) + y.powi(4) - squared);
    rounded <= 1.0 && y >= -1.0 + eye.upper_lid + eye.lid_tilt * x
}

fn foreground(pose: FacePose, monochrome: bool) -> Color {
    if monochrome {
        return Color::White;
    }
    let cool = [220.0, 237.0, 243.0];
    let warm = [255.0, 237.0, 210.0];
    let color: [u8; 3] = std::array::from_fn(|index| {
        let value = (cool[index] + (warm[index] - cool[index]) * pose.warmth.clamp(0.0, 1.0))
            * pose.brightness.clamp(0.0, 1.0);
        // 量化很小的冷暖变化，避免静止时因一位色差不断重写所有点阵。
        ((value.round() as u8) / 4) * 4
    });
    Color::Rgb {
        r: color[0],
        g: color[1],
        b: color[2],
    }
}

#[derive(Default)]
pub(crate) struct Renderer {
    previous: Frame,
    current: Frame,
    previous_color: Option<Color>,
    previous_pose: Option<(FacePose, Style)>,
    output: Vec<u8>,
    run: String,
    invalidated: bool,
}

impl Renderer {
    pub(crate) fn invalidate(&mut self) {
        self.invalidated = true;
    }

    pub(crate) fn draw(
        &mut self,
        out: &mut impl Write,
        size: (u16, u16),
        pose: FacePose,
        style: Style,
    ) -> io::Result<()> {
        if !self.invalidated
            && self.previous.size == size
            && self.previous_pose == Some((pose, style))
        {
            return Ok(());
        }
        self.current.clear(size)?;
        self.current.compose(pose, style.mode);
        if size.0 == 0 || size.1 == 0 {
            self.previous.clear(size)?;
            self.invalidate();
            return Ok(());
        }
        let full_redraw = self.invalidated || self.previous.size != size;
        let color = foreground(pose, style.monochrome);
        let recolor = self.previous_color != Some(color);
        self.output.clear();
        let mut changed = full_redraw;
        if full_redraw {
            queue!(
                self.output,
                ResetColor,
                Clear(ClearType::All),
                SetForegroundColor(color)
            )?;
        }
        for y in 0..size.1 {
            let row = usize::from(y) * usize::from(size.0);
            let differs = |x: u16| {
                let index = row + usize::from(x);
                let current = self.current.cells[index];
                let previous = if full_redraw {
                    ' '
                } else {
                    self.previous.cells[index]
                };
                current != previous || (recolor && current != ' ')
            };
            let mut x = 0;
            while x < size.0 {
                if !differs(x) {
                    x += 1;
                    continue;
                }
                if !changed {
                    changed = true;
                    queue!(self.output, SetForegroundColor(color))?;
                }
                let start = x;
                let mut end = x + 1;
                let mut scan = end;
                while scan < size.0 && scan - end <= 2 {
                    if differs(scan) {
                        end = scan + 1;
                    }
                    scan += 1;
                }
                self.run.clear();
                self.run
                    .extend(&self.current.cells[row + usize::from(start)..row + usize::from(end)]);
                queue!(self.output, MoveTo(start, y), Print(self.run.as_str()))?;
                x = end;
            }
        }
        if changed {
            queue!(self.output, ResetColor)?;
            out.write_all(&self.output)?;
            out.flush()?;
        }
        std::mem::swap(&mut self.previous, &mut self.current);
        self.previous_color = Some(color);
        self.previous_pose = Some((pose, style));
        self.invalidated = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{Companion, Input};

    fn frame(size: (u16, u16), pose: FacePose, mode: Mode) -> Frame {
        let mut frame = Frame::default();
        frame.clear(size).unwrap();
        frame.compose(pose, mode);
        frame
    }

    #[test]
    fn ordinary_frames_contain_only_eyes_and_no_status_text_or_shading() {
        let cells = frame((80, 24), FacePose::default(), Mode::Dots).cells;
        assert!(
            cells
                .iter()
                .all(|&ch| ch == ' ' || ('\u{2801}'..='\u{28ff}').contains(&ch))
        );
        assert!(cells.contains(&'⣿'));
        assert!(cells[..80 * 3].iter().all(|&ch| ch == ' '));
        assert!(cells[80 * 21..].iter().all(|&ch| ch == ' '));
        assert_eq!(cells[12 * 80 + 40], ' ');
    }

    #[test]
    fn tiny_empty_and_repeatedly_resized_terminals_are_safe() {
        let mut renderer = Renderer::default();
        for size in [
            (80, 24),
            (240, 8),
            (20, 80),
            (12, 4),
            (3, 2),
            (1, 1),
            (0, 0),
            (80, 24),
        ] {
            let mut output = Vec::new();
            renderer
                .draw(&mut output, size, FacePose::default(), Style::default())
                .unwrap();
            assert_eq!(
                renderer.previous.cells.len(),
                usize::from(size.0) * usize::from(size.1)
            );
            if size != (0, 0) {
                assert!(output.windows(4).any(|bytes| bytes == b"\x1b[2J"));
            }
        }
        assert!(Frame::default().clear((u16::MAX, u16::MAX)).is_err());
    }

    #[test]
    fn stationary_frames_produce_no_output_and_mode_changes_do_not_clear() {
        let mut renderer = Renderer::default();
        let mut output = Vec::new();
        renderer
            .draw(&mut output, (80, 24), FacePose::default(), Style::default())
            .unwrap();
        output.clear();
        renderer
            .draw(&mut output, (80, 24), FacePose::default(), Style::default())
            .unwrap();
        assert!(output.is_empty());
        renderer
            .draw(
                &mut output,
                (80, 24),
                FacePose::default(),
                Style {
                    mode: Mode::Blocks,
                    ..Style::default()
                },
            )
            .unwrap();
        assert!(!output.windows(4).any(|bytes| bytes == b"\x1b[2J"));
        assert!(
            !renderer
                .previous
                .cells
                .iter()
                .any(|ch| ('\u{2800}'..='\u{28ff}').contains(ch))
        );
    }

    #[test]
    fn input_mapping_uses_the_same_aspect_ratio_as_the_eyes() {
        let size = (80, 24);
        let layout = Layout::new(size);
        let pointer = layout.pointer(39, 11);
        assert!(pointer.x.abs() < 0.02 && pointer.y.abs() < 0.04);
        for size in [(240, 8), (20, 80), (80, 24)] {
            let layout = Layout::new(size);
            let eye = FacePose::default().eyes[0];
            assert!(
                eye.center.x.abs() * layout.unit + eye.radius.x * layout.unit
                    < f32::from(size.0) * 0.5
            );
            assert!(eye.radius.y * layout.unit * 0.5 < f32::from(size.1) * 0.5);
        }
    }

    #[test]
    fn subtle_eyelids_keep_a_filled_open_center() {
        let mut eye = FacePose::default().eyes[0];
        eye.upper_lid = 0.25;
        eye.lid_tilt = 0.3;
        assert!(contains(0.0, 0.0, eye));
        assert!(!contains(0.0, -0.9, eye));
        assert!(contains(0.0, 0.8, eye));
    }

    #[test]
    fn failed_output_does_not_commit_the_frame() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut renderer = Renderer::default();
        assert!(
            renderer
                .draw(&mut Broken, (80, 24), FacePose::default(), Style::default())
                .is_err()
        );
        assert!(renderer.previous_pose.is_none());
        renderer
            .draw(
                &mut io::sink(),
                (80, 24),
                FacePose::default(),
                Style::default(),
            )
            .unwrap();
    }

    #[test]
    fn braille_masks_are_exact_and_never_become_a_texture() {
        for mask in 1..=255 {
            assert_eq!(Mode::Dots.character(mask) as u32, 0x2800 + u32::from(mask));
        }
        assert_eq!(Mode::Dots.character(0), ' ');
        assert_eq!(Mode::Blocks.character(15), '█');
    }

    #[test]
    #[ignore = "手动测量，不把机器性能作为测试条件"]
    fn measure_render_throughput() {
        use std::time::{Duration, Instant};
        for mode in [Mode::Dots, Mode::Blocks] {
            for size in [(80, 24), (160, 48), (320, 100)] {
                let mut model = Companion::default();
                let mut renderer = Renderer::default();
                let mut output = Vec::new();
                let mut timings = Vec::new();
                let mut bytes = 0;
                for index in 0..600 {
                    let t = index as f32 / 60.0;
                    let pose = model.update(
                        1.0 / 60.0,
                        Input {
                            pointer: Some(Point::new((t * 2.0).sin(), (t * 1.1).cos() * 0.4)),
                            greet: index == 100,
                            ..Input::default()
                        },
                    );
                    output.clear();
                    let start = Instant::now();
                    renderer
                        .draw(
                            &mut output,
                            size,
                            pose,
                            Style {
                                mode,
                                ..Style::default()
                            },
                        )
                        .unwrap();
                    timings.push(start.elapsed());
                    bytes += output.len();
                }
                let total: Duration = timings.iter().sum();
                timings.sort_unstable();
                println!(
                    "{mode:?} {}x{}: avg={:.2}ms p95={:.2}ms bytes/frame={:.0}",
                    size.0,
                    size.1,
                    total.as_secs_f64() * 1000.0 / timings.len() as f64,
                    timings[timings.len() * 95 / 100].as_secs_f64() * 1000.0,
                    bytes as f64 / timings.len() as f64
                );
            }
        }
    }
}
