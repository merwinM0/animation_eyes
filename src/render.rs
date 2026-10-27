use std::io::{self, Write};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType},
};

use crate::animation::{Expression, EyePose, FacePose};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RenderMode {
    #[default]
    DenseDots,
    Braille,
    Blocks,
}

impl RenderMode {
    fn samples(self) -> &'static [(f64, f64, u8)] {
        match self {
            // Unicode 盲文点位不是逐行连续编号，底行分别使用第 7、8 位。
            Self::DenseDots | Self::Braille => &[
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
            // 仅替换八个采样点全部命中的字符格；轮廓仍保留 2×4 点阵精度。
            // 盲文字形的点距由字体决定，内部用高覆盖率密纹补足视觉密度。
            Self::DenseDots if mask == u8::MAX => '▓',
            // u8 掩码限定在 U+2800..=U+28FF 内，必然是合法 Unicode 标量。
            Self::DenseDots | Self::Braille => char::from_u32(0x2800 + u32::from(mask))
                .expect("all eight-dot braille masks are valid Unicode"),
            Self::Blocks => quadrant_char(mask),
        }
    }

    fn hints(self) -> &'static str {
        match self {
            Self::DenseDots => "  Space:next P:pause B:sparse Q:quit",
            Self::Braille => "  Space:next P:pause B:blocks Q:quit",
            Self::Blocks => "  Space:next P:pause B:dense Q:quit",
        }
    }
}

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

    fn compose(&mut self, scene: &Scene<'_>, mode: RenderMode) {
        let layout = Layout::new(self.width, self.height);
        if self.width == 0 || self.height == 0 {
            return;
        }
        if let Some(eyes) = layout.eyes {
            for (geometry, pose) in eyes.into_iter().zip(scene.pose.eyes) {
                self.draw_eye(geometry, pose, scene.pose.gaze_x, &layout, mode);
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
                    self.text(x, y, mode.hints(), available.saturating_sub(x));
                }
            }
        }
    }

    fn draw_eye(
        &mut self,
        geometry: EyeGeometry,
        pose: EyePose,
        gaze_x: f64,
        layout: &Layout,
        mode: RenderMode,
    ) {
        let center_x = geometry.center_x + gaze_x * geometry.radius_x * 0.4;
        let center_y = geometry.center_y + pose.offset_y * geometry.radius_y;
        let radius_x = geometry.radius_x * pose.scale_x;
        // 使用浮点半径保留亚字符精度，极小窗口下也保证采样厚度。
        let radius_y = (geometry.radius_y * pose.scale_y).max(0.3);
        let extent_y = radius_y * (1.0 + pose.tilt.abs());
        let min_x = (center_x - radius_x).floor().max(0.0) as u16;
        let max_x = (center_x + radius_x).ceil().min(f64::from(self.width)) as u16;
        let min_y = (center_y - extent_y).floor().max(0.0) as u16;
        let max_y = (center_y + extent_y)
            .ceil()
            .min(f64::from(layout.content_height)) as u16;
        let inv_radius_x = radius_x.recip();
        let inv_radius_y = radius_y.recip();
        let samples = mode.samples();
        let mut offsets = [(0.0, 0.0, 0); 8];
        // 每只眼睛只换算一次采样偏移，像素内循环不再重复做浮点除法。
        for (offset, &(x, y, bit)) in offsets.iter_mut().zip(samples) {
            let dx = x * inv_radius_x;
            *offset = (dx, y * inv_radius_y - pose.tilt * dx, bit);
        }
        for y in min_y..max_y {
            for x in min_x..max_x {
                let dx = (f64::from(x) - center_x) * inv_radius_x;
                let dy = (f64::from(y) - center_y) * inv_radius_y - pose.tilt * dx;
                let mut mask = 0;
                for &(offset_x, offset_y, bit) in &offsets[..samples.len()] {
                    if eye_contains(dx + offset_x, dy + offset_y) {
                        mask |= bit;
                    }
                }
                if mask != 0 {
                    self.put(x, y, mode.character(mask));
                }
            }
        }
    }
}

fn eye_contains(x: f64, y: f64) -> bool {
    x * x + y * y <= 1.0
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
    output: Vec<u8>,
    mode: RenderMode,
    invalidated: bool,
}

impl Renderer {
    pub(crate) fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            RenderMode::DenseDots => RenderMode::Braille,
            RenderMode::Braille => RenderMode::Blocks,
            RenderMode::Blocks => RenderMode::DenseDots,
        };
    }

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
        self.current.compose(scene, self.mode);
        let resized = self.previous.width != width || self.previous.height != height;
        if width == 0 || height == 0 {
            self.previous.resize(width, height);
            self.invalidate();
            return Ok(());
        }
        self.output.clear();
        let full_redraw = resized || self.invalidated;
        let mut changed = full_redraw;
        if full_redraw {
            queue!(
                self.output,
                ResetColor,
                Clear(ClearType::All),
                SetForegroundColor(Color::White)
            )?;
        }
        // 空格也参与差量绘制；合并至多两个未变字符的短间隔，减少光标控制序列。
        for y in 0..height {
            let row = usize::from(y) * usize::from(width);
            let differs = |x: u16| {
                let index = row + usize::from(x);
                let old = if full_redraw {
                    ' '
                } else {
                    self.previous.cells[index]
                };
                self.current.cells[index] != old
            };
            let mut x = 0;
            while x < width {
                if !differs(x) {
                    x += 1;
                    continue;
                }
                if !changed {
                    changed = true;
                    queue!(self.output, SetForegroundColor(Color::White))?;
                }
                let start_x = x;
                let mut end_x = x + 1;
                let mut scan_x = end_x;
                while scan_x < width && scan_x - end_x <= 2 {
                    if differs(scan_x) {
                        end_x = scan_x + 1;
                    }
                    scan_x += 1;
                }
                self.run.clear();
                self.run.extend(
                    &self.current.cells[row + usize::from(start_x)..row + usize::from(end_x)],
                );
                queue!(self.output, MoveTo(start_x, y), Print(self.run.as_str()))?;
                x = end_x;
            }
        }
        if changed {
            queue!(self.output, ResetColor)?;
            // 整帧先编码进可复用缓冲，再一次性提交，避免逐条指令触发终端 IO。
            out.write_all(&self.output)?;
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
        frame_with_mode(size, scene, RenderMode::default())
    }

    fn frame_with_mode(size: (u16, u16), scene: &Scene<'_>, mode: RenderMode) -> Frame {
        let mut frame = Frame::default();
        frame.resize(size.0, size.1);
        frame.compose(scene, mode);
        frame
    }

    #[derive(Default)]
    struct OutputStats {
        writes: usize,
        flushes: usize,
        bytes: usize,
    }

    impl Write for OutputStats {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.bytes += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
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
    fn open_eyes_are_filled_ellipses_instead_of_closed_curves() {
        assert!(eye_contains(0.0, 0.0));
        assert!(eye_contains(0.0, -0.8));
        assert!(eye_contains(0.0, 0.8));
        assert!(eye_contains(0.8, 0.0));
        assert!(eye_contains(-0.8, 0.0));
        assert!(!eye_contains(0.8, 0.8));
        assert!(!eye_contains(1.1, 0.0));
    }

    #[test]
    fn dense_dots_fill_only_the_interior_and_preserve_every_edge_mask() {
        assert_eq!(RenderMode::DenseDots.character(0), ' ');
        assert_eq!(RenderMode::DenseDots.character(255), '▓');
        for mask in 1..255 {
            assert_eq!(
                RenderMode::DenseDots.character(mask),
                RenderMode::Braille.character(mask)
            );
        }
        let dense = frame_with_mode((80, 24), &scene(), RenderMode::DenseDots);
        let sparse = frame_with_mode((80, 24), &scene(), RenderMode::Braille);
        let body_end = 80 * 23;
        assert!(dense.cells[..body_end].contains(&'▓'));
        assert!(
            dense.cells[..body_end]
                .iter()
                .any(|ch| ('\u{2801}'..='\u{28fe}').contains(ch))
        );
        for (&dense_cell, &sparse_cell) in dense.cells[..body_end]
            .iter()
            .zip(&sparse.cells[..body_end])
        {
            assert_eq!(dense_cell == ' ', sparse_cell == ' ');
            if dense_cell != sparse_cell {
                assert_eq!((dense_cell, sparse_cell), ('▓', '⣿'));
            }
        }
    }

    #[test]
    fn braille_uses_the_standard_eight_dot_positions() {
        let samples = RenderMode::Braille.samples();
        assert_eq!(samples.len(), 8);
        for (index, &(x, y, bit)) in samples.iter().enumerate() {
            assert_eq!(x, if index % 2 == 0 { 0.25 } else { 0.75 });
            assert_eq!(y, 0.125 + (index / 2) as f64 * 0.25);
            assert_eq!(bit, [1, 8, 2, 16, 4, 32, 64, 128][index]);
        }
        for mask in 1..=u8::MAX {
            assert_eq!(
                RenderMode::Braille.character(mask) as u32,
                0x2800 + u32::from(mask)
            );
        }
        assert_eq!(RenderMode::Braille.character(0), ' ');
        assert_eq!(RenderMode::Braille.character(255), '⣿');
        assert_eq!(RenderMode::Blocks.character(15), '█');
    }

    #[test]
    fn braille_resolves_vertical_motion_between_block_samples() {
        let sample_cell = |mode, center_y| {
            let mut frame = Frame::default();
            frame.resize(12, 6);
            let layout = Layout::new(12, 6);
            frame.draw_eye(
                EyeGeometry {
                    center_x: 4.5,
                    center_y,
                    radius_x: 1.0,
                    radius_y: 1.0,
                },
                EyePose {
                    scale_y: 0.1,
                    ..EyePose::default()
                },
                0.0,
                &layout,
                mode,
            );
            frame.cells[2 * 12 + 4]
        };
        assert_eq!(
            sample_cell(RenderMode::Blocks, 2.0),
            sample_cell(RenderMode::Blocks, 2.125)
        );
        assert_ne!(
            sample_cell(RenderMode::Braille, 2.0),
            sample_cell(RenderMode::Braille, 2.125)
        );
    }

    #[test]
    fn mode_switch_replaces_old_glyphs_without_clearing_the_screen() {
        let mut renderer = Renderer::default();
        let mut output = Vec::new();
        for (index, mode) in [
            RenderMode::DenseDots,
            RenderMode::Braille,
            RenderMode::Blocks,
            RenderMode::DenseDots,
        ]
        .into_iter()
        .enumerate()
        {
            output.clear();
            renderer.draw(&mut output, (80, 24), &scene()).unwrap();
            assert_eq!(renderer.mode, mode);
            assert_eq!(
                renderer.previous.cells,
                frame_with_mode((80, 24), &scene(), mode).cells
            );
            if index > 0 {
                assert!(!output.windows(4).any(|bytes| bytes == b"\x1b[2J"));
            }
            if mode != RenderMode::DenseDots {
                assert!(!renderer.previous.cells.contains(&'▓'));
            }
            if mode == RenderMode::Blocks {
                assert!(
                    !renderer
                        .previous
                        .cells
                        .iter()
                        .any(|ch| ('\u{2800}'..='\u{28ff}').contains(ch))
                );
            }
            renderer.toggle_mode();
        }
    }

    #[test]
    fn changed_frames_are_submitted_with_one_write_and_one_flush() {
        let mut renderer = Renderer::default();
        let mut stats = OutputStats::default();
        renderer.draw(&mut stats, (160, 48), &scene()).unwrap();
        assert_eq!((stats.writes, stats.flushes), (1, 1));
        renderer.draw(&mut stats, (160, 48), &scene()).unwrap();
        assert_eq!((stats.writes, stats.flushes), (1, 1));
        renderer
            .draw(
                &mut stats,
                (160, 48),
                &Scene {
                    time: "12:35",
                    ..scene()
                },
            )
            .unwrap();
        assert_eq!((stats.writes, stats.flushes), (2, 2));
    }

    #[test]
    fn nearby_changes_share_one_cursor_move() {
        let mut renderer = Renderer::default();
        let mut output = Vec::new();
        renderer.draw(&mut output, (80, 24), &scene()).unwrap();
        output.clear();
        renderer
            .draw(
                &mut output,
                (80, 24),
                &Scene {
                    time: "13:35",
                    ..scene()
                },
            )
            .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("\x1b[24;77H3:35"));
        assert_eq!(output.matches('H').count(), 1);
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
    fn flush_errors_do_not_commit_an_unfinished_frame() {
        struct FailedFlush;
        impl Write for FailedFlush {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "flush failed"))
            }
        }
        let mut renderer = Renderer::default();
        assert_eq!(
            renderer
                .draw(&mut FailedFlush, (80, 24), &scene())
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert!(renderer.previous.cells.is_empty());
        renderer.draw(&mut io::sink(), (80, 24), &scene()).unwrap();
        assert_eq!(renderer.previous.cells, frame((80, 24), &scene()).cells);
    }

    #[test]
    #[ignore = "手动测量渲染耗时与输出量，不在正常测试中断言机器性能"]
    fn measure_render_throughput() {
        use std::time::{Duration, Instant};

        for mode in [
            RenderMode::Blocks,
            RenderMode::Braille,
            RenderMode::DenseDots,
        ] {
            for size in [(80, 24), (160, 48), (320, 100)] {
                let mut renderer = Renderer {
                    mode,
                    ..Renderer::default()
                };
                let mut stats = OutputStats::default();
                let mut animation = AnimationState::new();
                let frame_count = Expression::ALL.len() * 120;
                let mut timings = Vec::with_capacity(frame_count);
                for index in 0..frame_count {
                    let t = index as f64 / 60.0;
                    if index % 60 == 0 {
                        animation.next_expression(t);
                    }
                    let mut pose = animation.sample(t);
                    pose.gaze_x = (t * 3.0).sin();
                    let scene = Scene {
                        pose,
                        expression: animation.expression(),
                        ..scene()
                    };
                    let start = Instant::now();
                    renderer.draw(&mut stats, size, &scene).unwrap();
                    timings.push(start.elapsed());
                    std::hint::black_box(&renderer.previous.cells);
                }
                let total: Duration = timings.iter().sum();
                timings.sort_unstable();
                println!(
                    "{mode:?} {}x{}: avg={:.1}us p95={:.1}us max={:.1}us bytes/frame={:.1}",
                    size.0,
                    size.1,
                    total.as_secs_f64() * 1e6 / timings.len() as f64,
                    timings[timings.len() * 95 / 100].as_secs_f64() * 1e6,
                    timings.last().unwrap().as_secs_f64() * 1e6,
                    stats.bytes as f64 / timings.len() as f64
                );
            }
        }
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
