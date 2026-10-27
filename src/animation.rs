use std::{
    f32::consts::TAU,
    ops::{Add, Mul, Sub},
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Point {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

impl Point {
    pub(crate) const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub(crate) fn length(self) -> f32 {
        self.x.hypot(self.y)
    }
    fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Add for Point {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}
impl Sub for Point {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}
impl Mul<f32> for Point {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EyePose {
    pub(crate) center: Point,
    pub(crate) radius: Point,
    pub(crate) tilt: f32,
    pub(crate) roundness: f32,
    pub(crate) upper_lid: f32,
    pub(crate) lid_tilt: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FacePose {
    pub(crate) eyes: [EyePose; 2],
    pub(crate) warmth: f32,
    pub(crate) brightness: f32,
}

impl Default for FacePose {
    fn default() -> Self {
        Self {
            eyes: [-1.0, 1.0].map(|side| EyePose {
                center: Point::new(side * 0.45, 0.0),
                radius: Point::new(0.26, 0.32),
                tilt: 0.0,
                roundness: 0.3,
                upper_lid: 0.0,
                lid_tilt: 0.0,
            }),
            warmth: 0.2,
            brightness: 0.9,
        }
    }
}

/// 输入仅来自当前终端可视区；指针使用与眼睛相同的等比例世界坐标。
#[derive(Clone, Copy, Debug)]
pub(crate) struct Input {
    pub(crate) pointer: Option<Point>,
    pub(crate) focused: bool,
    pub(crate) contact: bool,
    pub(crate) greet: bool,
    pub(crate) soothe: bool,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            pointer: None,
            focused: true,
            contact: false,
            greet: false,
            soothe: false,
        }
    }
}

/// 临界阻尼的解析解：短帧和长帧使用同一种运动，不依赖固定步数或追赶旧帧。
#[derive(Clone, Copy, Debug, Default)]
struct Spring {
    value: f32,
    velocity: f32,
}

impl Spring {
    fn at(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
        }
    }
    fn follow(&mut self, target: f32, omega: f32, dt: f32) {
        let offset = self.value - target;
        let impulse = (self.velocity + omega * offset) * dt;
        let decay = (-omega * dt).exp();
        self.value = target + (offset + impulse) * decay;
        self.velocity = (self.velocity - omega * impulse) * decay;
    }
}

pub(crate) struct Companion {
    gaze: [Spring; 2],
    warmth: Spring,
    attention: Spring,
    curiosity: Spring,
    caution: Spring,
    delight: Spring,
    trust: f32,
    comfort: f32,
    greeting: f32,
    previous_pointer: Option<Point>,
    was_focused: bool,
    away_for: f32,
    phase: f32,
    reduced_motion: bool,
}

impl Default for Companion {
    fn default() -> Self {
        Self {
            gaze: [Spring::default(); 2],
            warmth: Spring::at(0.22),
            attention: Spring::at(0.35),
            curiosity: Spring::default(),
            caution: Spring::default(),
            delight: Spring::default(),
            trust: 0.2,
            comfort: 0.0,
            greeting: 0.0,
            previous_pointer: None,
            was_focused: true,
            away_for: 0.0,
            phase: 0.0,
            reduced_motion: false,
        }
    }
}

impl Companion {
    pub(crate) fn set_reduced_motion(&mut self, enabled: bool) {
        self.reduced_motion = enabled;
    }

    pub(crate) fn update(&mut self, elapsed: f32, input: Input) -> FacePose {
        // 最小化、拖动或调试器恢复后不把整段离线时间灌进模拟。
        let dt = if elapsed.is_finite() {
            elapsed.clamp(0.0, 0.1)
        } else {
            0.0
        };
        if dt == 0.0 {
            return self.pose();
        }
        let pointer = input
            .pointer
            .filter(|point| input.focused && point.finite())
            .map(|point| Point::new(point.x.clamp(-3.0, 3.0), point.y.clamp(-3.0, 3.0)));
        let speed = match (pointer, self.previous_pointer) {
            (Some(current), Some(previous)) => ((current - previous).length() / dt).min(30.0),
            _ => 0.0,
        };
        self.previous_pointer = pointer;

        // 重新聚焦终端是一次真正的交互事件，而不是周期性播放的“欢迎表情”。
        if input.focused && !self.was_focused && self.away_for > 1.0 {
            self.greeting = self.greeting.max(0.8);
        }
        if input.focused {
            self.away_for = 0.0;
        } else {
            self.away_for = (self.away_for + dt).min(3600.0);
        }
        self.was_focused = input.focused;
        if input.focused && input.greet {
            self.greeting = 1.0;
            self.comfort = (self.comfort + 0.18).min(1.0);
        }

        let proximity = pointer.map_or(0.0, |pointer| {
            self.pose()
                .eyes
                .iter()
                .map(|eye| {
                    let delta = pointer - eye.center;
                    1.0 - Point::new(delta.x / 0.38, delta.y / 0.44)
                        .length()
                        .clamp(0.0, 1.0)
                })
                .fold(0.0, f32::max)
        });
        let gentle_motion = smoothstep(0.015, 0.15, speed) * (1.0 - smoothstep(1.2, 3.0, speed));
        let stroking = proximity * gentle_motion;
        let contact = if input.contact { proximity } else { 0.0 };
        let soothe = if input.focused && input.soothe {
            1.0
        } else {
            0.0
        };
        let kindness = stroking.max(contact).max(soothe);
        self.comfort = approach(
            self.comfort,
            kindness,
            if kindness > self.comfort { 1.8 } else { 0.16 },
            dt,
        );
        self.trust =
            (self.trust + (kindness * 0.065 - (1.0 - kindness) * 0.0003) * dt).clamp(0.0, 1.0);
        self.greeting *= (-dt / 1.4).exp();

        let quick_motion = smoothstep(3.0, 10.0, speed);
        let presence = if pointer.is_some() { 1.0 } else { 0.0 };
        let focused = if input.focused { 1.0 } else { 0.0 };
        self.warmth.follow(
            (0.12 + self.trust * 0.42 + self.comfort * 0.46).clamp(0.0, 1.0),
            3.0,
            dt,
        );
        self.attention.follow(
            (0.16 + focused * 0.18 + presence * 0.2 + self.greeting * 0.35).min(1.0),
            5.0,
            dt,
        );
        self.curiosity.follow(
            (gentle_motion * 0.45
                + proximity * 0.3
                + self.greeting * 0.2
                + pointer.map_or(0.0, |point| point.x.abs().min(1.0) * 0.28))
            .min(1.0),
            4.5,
            dt,
        );
        self.caution
            .follow(quick_motion * (1.0 - self.trust * 0.45), 7.0, dt);
        self.delight.follow(self.greeting, 9.0, dt);

        let target = pointer.unwrap_or_default();
        let response = if self.reduced_motion { 5.0 } else { 11.0 };
        self.gaze[0].follow((target.x * 0.75).clamp(-1.0, 1.0), response, dt);
        self.gaze[1].follow((target.y * 0.95).clamp(-1.0, 1.0), response, dt);
        self.phase = (self.phase + dt * TAU / 4.6) % TAU;
        self.pose()
    }

    pub(crate) fn pose(&self) -> FacePose {
        let warmth = self.warmth.value.clamp(0.0, 1.0);
        let attention = self.attention.value.clamp(0.0, 1.0);
        let curious = self.curiosity.value.clamp(0.0, 1.0);
        let caution = self.caution.value.clamp(0.0, 1.0);
        let delight = self.delight.value.clamp(0.0, 1.0);
        let gaze = Point::new(
            self.gaze[0].value.clamp(-1.0, 1.0),
            self.gaze[1].value.clamp(-1.0, 1.0),
        );
        let breath = if self.reduced_motion {
            0.0
        } else {
            self.phase.sin()
        };
        let eyes = [-1.0_f32, 1.0].map(|side| {
            let interest = (side * gaze.x).max(0.0) * curious;
            let asymmetry = side * curious;
            EyePose {
                center: Point::new(
                    side * (0.45 + delight * 0.025 - warmth * 0.012) + gaze.x * 0.11,
                    gaze.y * 0.085 - warmth * 0.018 - delight * 0.035
                        + asymmetry * 0.035
                        + breath * 0.006,
                ),
                radius: Point::new(
                    0.26 * (1.0 + warmth * 0.05 + delight * 0.09 - caution * 0.025),
                    0.32 * (1.0
                        + delight * 0.24
                        + attention * 0.04
                        + interest * 0.22
                        + asymmetry * 0.12
                        - caution * 0.10
                        + breath * 0.009)
                        .clamp(0.85, 1.45),
                ),
                tilt: side * (caution * 0.19 - warmth * 0.075) + gaze.x * curious * 0.12,
                roundness: (0.28 + warmth * 0.28 - delight * 0.22 + caution * 0.1).clamp(0.0, 0.8),
                // 眼睑只轻微改变上沿，不收成眯眼或闭眼线条。
                upper_lid: (caution * 0.18 + (1.0 - interest) * curious * 0.07).min(0.25),
                lid_tilt: side * caution * 0.25 + gaze.x * curious * 0.12,
            }
        });
        FacePose {
            eyes,
            warmth,
            brightness: 0.72 + attention * 0.20 + delight * 0.08,
        }
    }
}

fn approach(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    target + (current - target) * (-rate * dt).exp()
}

fn smoothstep(low: f32, high: f32, value: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(model: &mut Companion, seconds: f32, input: Input) -> FacePose {
        for _ in 0..(seconds * 120.0) as usize {
            model.update(1.0 / 120.0, input);
        }
        model.pose()
    }

    #[test]
    fn gaze_tracks_local_pointer_and_returns_when_it_leaves() {
        let mut model = Companion::default();
        let looking = run(
            &mut model,
            2.0,
            Input {
                pointer: Some(Point::new(1.0, -0.5)),
                ..Input::default()
            },
        );
        assert!(looking.eyes[0].center.x > -0.40);
        assert!(looking.eyes[0].center.y < -0.02);
        run(&mut model, 3.0, Input::default());
        assert!(model.gaze[0].value.abs() < 0.001);
        assert!(model.gaze[1].value.abs() < 0.001);
    }

    #[test]
    fn gentle_contact_builds_warmth_and_memory_that_fades_slowly() {
        let mut model = Companion::default();
        let initial = model.pose().warmth;
        let warm = run(
            &mut model,
            8.0,
            Input {
                pointer: Some(Point::new(-0.45, 0.0)),
                contact: true,
                ..Input::default()
            },
        );
        assert!(warm.warmth > initial + 0.3);
        let after = run(&mut model, 2.0, Input::default());
        assert!(after.warmth > initial + 0.2);
    }

    #[test]
    fn keyboard_soothing_is_an_equivalent_alternative_to_mouse_contact() {
        let mut model = Companion::default();
        let pose = run(
            &mut model,
            6.0,
            Input {
                soothe: true,
                ..Input::default()
            },
        );
        assert!(pose.warmth > 0.65);
    }

    #[test]
    fn greeting_is_an_impulse_with_a_smooth_response_not_a_pose_jump() {
        let mut model = Companion::default();
        let before = model.pose();
        let first = model.update(
            1.0 / 60.0,
            Input {
                greet: true,
                ..Input::default()
            },
        );
        assert!((first.eyes[0].radius.y - before.eyes[0].radius.y).abs() < 0.01);
        let greeting = run(&mut model, 0.5, Input::default());
        assert!(greeting.eyes[0].radius.y > before.eyes[0].radius.y + 0.025);
        run(&mut model, 8.0, Input::default());
        assert!(model.delight.value < 0.02);
    }

    #[test]
    fn rapid_movement_produces_caution_while_gentle_strokes_produce_warmth() {
        let mut model = Companion::default();
        for index in 0..120 {
            model.update(
                1.0 / 60.0,
                Input {
                    pointer: Some(Point::new(if index % 2 == 0 { -0.5 } else { 0.5 }, 0.0)),
                    ..Input::default()
                },
            );
        }
        assert!(model.caution.value > 0.6);
        run(&mut model, 4.0, Input::default());
        assert!(model.caution.value < 0.001);
    }

    #[test]
    fn only_a_real_focus_return_produces_a_welcome() {
        let mut model = Companion::default();
        run(&mut model, 60.0, Input::default());
        assert_eq!(model.greeting, 0.0);
        run(
            &mut model,
            3.0,
            Input {
                focused: false,
                ..Input::default()
            },
        );
        model.update(1.0 / 60.0, Input::default());
        assert!(model.greeting > 0.7);
    }

    #[test]
    fn unfocused_terminal_does_not_react_to_pointer_or_contact() {
        let mut model = Companion::default();
        run(
            &mut model,
            10.0,
            Input {
                focused: false,
                pointer: Some(Point::new(0.45, 0.0)),
                contact: true,
                greet: true,
                soothe: true,
            },
        );
        assert_eq!(model.greeting, 0.0);
        assert!(model.comfort < 0.001);
        assert!(model.gaze[0].value.abs() < 0.001);
    }

    #[test]
    fn spring_solution_is_independent_of_frame_rate() {
        let mut a = Spring::default();
        let mut b = Spring::default();
        for _ in 0..60 {
            a.follow(1.0, 8.0, 1.0 / 60.0);
        }
        for _ in 0..144 {
            b.follow(1.0, 8.0, 1.0 / 144.0);
        }
        assert!((a.value - b.value).abs() < 1e-5);
        assert!((a.velocity - b.velocity).abs() < 1e-5);
    }

    #[test]
    fn reduced_motion_removes_idle_breathing() {
        let mut model = Companion::default();
        model.set_reduced_motion(true);
        run(&mut model, 20.0, Input::default());
        let a = model.pose();
        let b = run(&mut model, 1.0, Input::default());
        for (a, b) in a.eyes.into_iter().zip(b.eyes) {
            assert!((a.center.y - b.center.y).abs() < 1e-5);
            assert!((a.radius.y - b.radius.y).abs() < 1e-5);
        }
    }

    #[test]
    fn invalid_input_and_long_stalls_cannot_poison_the_model() {
        let mut model = Companion::default();
        model.update(f32::NAN, Input::default());
        model.update(f32::INFINITY, Input::default());
        model.update(
            10000.0,
            Input {
                pointer: Some(Point::new(f32::NAN, 0.0)),
                ..Input::default()
            },
        );
        assert!(model.pose().eyes[0].center.finite());
        assert!(model.phase < 0.2);
    }

    #[test]
    fn every_interaction_keeps_both_eyes_open_and_geometry_bounded() {
        let mut model = Companion::default();
        for index in 0..12000 {
            let t = index as f32 / 60.0;
            let input = Input {
                pointer: Some(Point::new((t * 8.0).sin(), (t * 3.0).cos())),
                focused: index % 500 < 400,
                contact: index % 180 < 90,
                greet: index % 100 == 0,
                soothe: index % 350 < 80,
            };
            let pose = model.update(1.0 / 60.0, input);
            for eye in pose.eyes {
                assert!(eye.center.finite() && eye.radius.finite());
                assert!((0.32 * 0.85..0.48).contains(&eye.radius.y));
                assert!((0.24..0.31).contains(&eye.radius.x));
                assert!(eye.center.x.abs() + eye.radius.x < 1.0);
                assert!(eye.center.y.abs() + eye.radius.y * (1.0 + eye.tilt.abs()) < 0.725);
            }
        }
    }
}
