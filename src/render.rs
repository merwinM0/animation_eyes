use std::io::{self, Write};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType},
};

use crate::animation::{Expression, EyePose, EyeShape, FacePose};

pub(crate) struct Scene<'a> {
    pub(crate) pose: FacePose,
    pub(crate) expression: Option<Expression>,
    pub(crate) paused: bool,
    pub(crate) time: &'a str,
}

#[derive(Debug)]
struct Layout {
    content_height: u16,
    footer_y: Option<u16>,
    eyes: Option<[EyeGeometry; 2]>,
}

#[derive(Clone, Copy, Debug)]
struct EyeGeometry {
    center_x: f64,
    center_y: f64,
    radius_x: f64,
    radius_y: f64,
}

impl Layout {
    fn new(width: u16, height: u16) -> Self {
        let footer_y = (width >= 5 && height >= 3).then(|| height - 1);
        let content_height = height - u16::from(footer_y.is_some());
        let eyes = if width >= 12 && content_height >= 4 {
            let w = f64::from(width);
            let h = f64::from(content_height);
            // 为最大表情、上下移动及底栏留出空间，宽而矮的终端也不会裁掉眼睛。
            let radius = (w / 12.0).min(h / 4.0);
            let margin = radius * 1.6;
            let center_y = (h * 0.4).clamp(margin, h - margin);
            Some([
                EyeGeometry {
                    center_x: w / 2.0 - radius * 2.0,
                    center_y,
                    radius_x: radius,
                    radius_y: radius,
                },
                EyeGeometry {
                    center_x: w / 2.0 + radius * 2.0,
                    center_y,
                    radius_x: radius,
                    radius_y: radius,
                },
            ])
        } else {
            None
        };
        Self {
            content_height,
            footer_y,
            eyes,
        }
    }
}

#[derive(Default)]
struct Frame {
    width: u16,
    height: u16,
    cells: Vec<char>,
}

impl Frame {
    fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.cells
            .resize(usize::from(width) * usize::from(height), ' ');
        self.cells.fill(' ');
    }

    fn put(&mut self, x: u16, y: u16, value: char) {
        if x < self.width && y < self.height {
            self.cells[usize::from(y) * usize::from(self.width) + usize::from(x)] = value;
        }
    }

    // 所有底栏和紧凑表情都使用单列 ASCII，避免窄终端的中英文宽度歧义。
    fn text(&mut self, x: u16, y: u16, text: &str, max_width: u16) {
        let available = self.width.saturating_sub(x).min(max_width);
        for (offset, ch) in text.chars().take(usize::from(available)).enumerate() {
            self.put(x + offset as u16, y, ch);
        }
    }

    fn compose(&mut self, scene: &Scene<'_>) {
        let layout = Layout::new(self.width, self.height);
        if self.width == 0 || self.height == 0 {
            return;
        }
        if let Some(eyes) = layout.eyes {
            for (geometry, pose) in eyes.into_iter().zip(scene.pose.eyes) {
                self.draw_eye(geometry, pose, scene.pose.gaze_x, &layout);
            }
        } else if layout.content_height > 0 {
            let compact = scene.expression.map_or("o_o", Expression::compact);
            let text_width = (compact.len() as u16).min(self.width);
            self.text(
                (self.width - text_width) / 2,
                layout.content_height / 2,
                compact,
                text_width,
            );
        }
        if let Some(y) = layout.footer_y {
            let time_width = scene.time.len().min(usize::from(u16::MAX)) as u16;
            if time_width <= self.width {
                self.text(self.width - time_width, y, scene.time, time_width);
                let available = self.width.saturating_sub(time_width).saturating_sub(1);
                let label = scene.expression.map_or("Idle", Expression::label);
                self.text(0, y, label, available);
                let mut x = label.len() as u16;
                if scene.paused {
                    self.text(x, y, " [paused]", available.saturating_sub(x));
                    x += 9;
                }
                if self.width >= 64 {
                    self.text(
                        x,
                        y,
                        "  Space:next P:pause Q:quit",
                        available.saturating_sub(x),
                    );
                }
            }
        }
    }

    fn draw_eye(&mut self, geometry: EyeGeometry, pose: EyePose, gaze_x: f64, layout: &Layout) {
        let center_x = geometry.center_x + gaze_x * geometry.radius_x * 0.4;
        let center_y = geometry.center_y + pose.offset_y * geometry.radius_y;
        let radius_x = geometry.radius_x * pose.scale_x;
        // 闭眼保留亚字符厚度，不再被整数半径的最小值卡成睁眼状态。
        let radius_y = (geometry.radius_y * pose.scale_y).max(0.3);
        let extent_y = radius_y * (1.0 + pose.tilt.abs());
        let min_x = (center_x - radius_x).floor().max(0.0) as u16;
        let max_x = (center_x + radius_x).ceil().min(f64::from(self.width)) as u16;
        let min_y = (center_y - extent_y).floor().max(0.0) as u16;
        let max_y = (center_y + extent_y)
            .ceil()
            .min(f64::from(layout.content_height)) as u16;
        const SAMPLES: [(f64, f64, u8); 4] = [
            (0.25, 0.25, 1),
            (0.75, 0.25, 2),
            (0.25, 0.75, 4),
            (0.75, 0.75, 8),
        ];
        for y in min_y..max_y {
            for x in min_x..max_x {
                let mut mask = 0;
                for (offset_x, offset_y, bit) in SAMPLES {
                    let dx = (f64::from(x) + offset_x - center_x) / radius_x;
                    let dy = (f64::from(y) + offset_y - center_y) / radius_y - pose.tilt * dx;
                    if eye_contains(dx, dy, pose) {
                        mask |= bit;
                    }
                }
                if mask != 0 {
                    self.put(x, y, quadrant_char(mask));
                }
            }
        }
    }
}

fn eye_contains(x: f64, y: f64, pose: EyePose) -> bool {
    let oval = x * x + y * y - 1.0;
    let target = match pose.shape {
        EyeShape::Oval => oval,
        EyeShape::Happy | EyeShape::Sad => {
            let curve = if pose.shape == EyeShape::Happy {
                -0.35 + 0.8 * x * x
            } else {
                0.35 - 0.8 * x * x
            };
            ((y - curve).abs() - 0.18).max(oval)
        }
        EyeShape::Heart => {
            // 隐式爱心的顶部超过单位圆，缩入边界内以免两个心瓣被裁平。
            let hx = x * 1.3;
            let hy = -y * 1.3;
            (hx * hx + hy * hy - 1.0).powi(3) - hx * hx * hy.powi(3)
        }
    };
    oval * (1.0 - pose.blend) + target * pose.blend <= 0.0
}

fn quadrant_char(mask: u8) -> char {
    const QUADRANTS: [char; 16] = [
        ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
    ];
    QUADRANTS[usize::from(mask)]
}

#[derive(Default)]
pub(crate) struct Renderer {
    previous: Frame,
    current: Frame,
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
        scene: &Scene<'_>,
    ) -> io::Result<()> {
        let (width, height) = size;
        self.current.resize(width, height);
        self.current.compose(scene);
        let resized = self.previous.width != width || self.previous.height != height;
        if width == 0 || height == 0 {
            self.previous.resize(width, height);
            self.invalidate();
            return Ok(());
        }
        let mut changed = resized || self.invalidated;
        if changed {
            queue!(
                out,
                ResetColor,
                Clear(ClearType::All),
                SetForegroundColor(Color::White)
            )?;
            self.previous.resize(width, height);
        }
        // 按行合并相邻变化，空格也参与差量绘制，负责擦除上一帧的残影。
        for y in 0..height {
            let row = usize::from(y) * usize::from(width);
            let mut x = 0;
            while x < width {
                let index = row + usize::from(x);
                if self.current.cells[index] == self.previous.cells[index] {
                    x += 1;
                    continue;
                }
                if !changed {
                    changed = true;
                    queue!(out, SetForegroundColor(Color::White))?;
                }
                let start_x = x;
                self.run.clear();
                while x < width {
                    let index = row + usize::from(x);
                    if self.current.cells[index] == self.previous.cells[index] {
                        break;
                    }
                    self.run.push(self.current.cells[index]);
                    x += 1;
                }
                queue!(out, MoveTo(start_x, y), Print(self.run.as_str()))?;
            }
        }
        if changed {
            queue!(out, ResetColor)?;
            out.flush()?;
        }
        std::mem::swap(&mut self.previous, &mut self.current);
        self.invalidated = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::AnimationState;

    fn scene() -> Scene<'static> {
        Scene {
            pose: FacePose::default(),
            expression: None,
            paused: false,
            time: "12:34",
        }
    }

    fn frame(size: (u16, u16), scene: &Scene<'_>) -> Frame {
        let mut frame = Frame::default();
        frame.resize(size.0, size.1);
        frame.compose(scene);
        frame
    }

    #[test]
    fn layout_keeps_all_poses_inside_the_content_area() {
        for (width, height) in [
            (12, 5),
            (80, 24),
            (240, 8),
            (20, 80),
            (400, 120),
            (u16::MAX, 5),
        ] {
            let layout = Layout::new(width, height);
            let eyes = layout.eyes.expect("large enough for full eyes");
            let mut animation = AnimationState::new();
            for _ in Expression::ALL {
                animation.next_expression(0.0);
                let face = animation.sample(0.0);
                for (geometry, pose) in eyes.into_iter().zip(face.eyes) {
                    let center_y = geometry.center_y + pose.offset_y * geometry.radius_y;
                    let extent =
                        (geometry.radius_y * pose.scale_y).max(0.3) * (1.0 + pose.tilt.abs());
                    assert!(center_y - extent >= 0.0, "{width}x{height}");
                    assert!(
                        center_y + extent <= f64::from(layout.content_height),
                        "{width}x{height}"
                    );
                    for gaze in [-1.0, 1.0] {
                        let x = geometry.center_x + gaze * geometry.radius_x * 0.4;
                        assert!(x - geometry.radius_x * pose.scale_x >= 0.0);
                        assert!(x + geometry.radius_x * pose.scale_x <= f64::from(width));
                    }
                }
            }
        }
    }

    #[test]
    fn wide_short_windows_scale_the_entire_face_without_flattening_eyes() {
        for (width, height) in [(240, 8), (400, 5), (80, 24)] {
            let eyes = Layout::new(width, height).eyes.unwrap();
            for eye in eyes {
                assert_eq!(eye.radius_x, eye.radius_y);
            }
            assert!((eyes[0].center_x + eyes[1].center_x - f64::from(width)).abs() < 1e-10);
            assert!((eyes[1].center_x - eyes[0].center_x - eyes[0].radius_x * 4.0).abs() < 1e-10);
        }
    }

    #[test]
    fn tiny_and_empty_terminals_are_safe() {
        for width in 0..16 {
            for height in 0..8 {
                let frame = frame((width, height), &scene());
                assert_eq!(frame.cells.len(), usize::from(width) * usize::from(height));
                if width > 0 && height > 0 {
                    assert!(frame.cells.iter().any(|&ch| ch != ' '));
                }
            }
        }
    }

    #[test]
    fn narrow_clock_is_hidden_and_full_clock_stays_on_the_last_row() {
        let narrow = frame((4, 3), &scene());
        assert!(!narrow.cells.contains(&':'));
        for size in [(5, 3), (12, 5), (80, 24), (240, 8)] {
            let frame = frame(size, &scene());
            let clock: String = frame.cells[frame.cells.len() - 5..].iter().collect();
            assert_eq!(clock, "12:34");
        }
    }

    #[test]
    fn shapes_have_distinct_silhouettes() {
        let mut animation = AnimationState::new();
        let mut silhouettes = Vec::new();
        for _ in Expression::ALL {
            animation.next_expression(0.0);
            let mut scene = scene();
            scene.pose = animation.sample(0.0);
            let frame = frame((120, 40), &scene);
            if !silhouettes.contains(&frame.cells) {
                silhouettes.push(frame.cells);
            }
        }
        assert_eq!(silhouettes.len(), Expression::ALL.len());
    }

    #[test]
    fn heart_has_two_lobes_and_a_lower_point() {
        let heart = EyePose {
            shape: EyeShape::Heart,
            blend: 1.0,
            ..EyePose::default()
        };
        assert!(!eye_contains(0.0, -0.9, heart));
        assert!(eye_contains(0.4, -0.7, heart));
        assert!(eye_contains(-0.4, -0.7, heart));
        assert!(eye_contains(0.0, 0.75, heart));
        assert!(!eye_contains(0.6, 0.75, heart));
        for step in -100..=100 {
            let coordinate = f64::from(step) / 100.0;
            for (x, y) in [
                (coordinate, -1.0),
                (coordinate, 1.0),
                (-1.0, coordinate),
                (1.0, coordinate),
            ] {
                assert!(!eye_contains(x, y, heart), "heart exceeds its bounding box");
            }
        }
    }

    #[test]
    fn unchanged_frame_produces_no_terminal_output() {
        let mut renderer = Renderer::default();
        let mut output = Vec::new();
        renderer.draw(&mut output, (80, 24), &scene()).unwrap();
        assert!(output.windows(4).any(|bytes| bytes == b"\x1b[2J"));
        output.clear();
        renderer.draw(&mut output, (80, 24), &scene()).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn clock_update_only_writes_the_changed_character() {
        let mut renderer = Renderer::default();
        let mut output = Vec::new();
        renderer.draw(&mut output, (80, 24), &scene()).unwrap();
        output.clear();
        renderer
            .draw(
                &mut output,
                (80, 24),
                &Scene {
                    time: "12:35",
                    ..scene()
                },
            )
            .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("\x1b[24;80H5"));
        assert!(!output.contains("\x1b[2J"));
        assert!(!output.contains('█'));
    }

    #[test]
    fn shrinking_growing_and_invalidating_force_a_clean_redraw() {
        let mut renderer = Renderer::default();
        let mut output = Vec::new();
        for size in [(120, 40), (20, 6), (1, 1), (0, 0), (80, 24), (240, 8)] {
            output.clear();
            renderer.draw(&mut output, size, &scene()).unwrap();
            if size != (0, 0) {
                assert!(output.windows(4).any(|bytes| bytes == b"\x1b[2J"));
            }
            assert_eq!(renderer.previous.cells, frame(size, &scene()).cells);
        }
        output.clear();
        renderer.invalidate();
        renderer.draw(&mut output, (240, 8), &scene()).unwrap();
        assert!(output.windows(4).any(|bytes| bytes == b"\x1b[2J"));
    }

    #[test]
    fn motion_erases_old_pixels_without_clearing_the_screen() {
        let mut renderer = Renderer::default();
        let mut output = Vec::new();
        renderer.draw(&mut output, (80, 24), &scene()).unwrap();
        output.clear();
        let moved = Scene {
            pose: FacePose {
                gaze_x: 1.0,
                ..FacePose::default()
            },
            ..scene()
        };
        renderer.draw(&mut output, (80, 24), &moved).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains(' '));
        assert!(!output.contains("\x1b[2J"));
        assert_eq!(renderer.previous.cells, frame((80, 24), &moved).cells);
    }

    #[test]
    fn write_errors_are_propagated() {
        struct BrokenWriter;
        impl Write for BrokenWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let error = Renderer::default()
            .draw(&mut BrokenWriter, (80, 24), &scene())
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
