use std::{
    f64::consts::PI,
    time::{SystemTime, UNIX_EPOCH},
};

const MOVE_WINDOW_SECONDS: f64 = 25.0;
const MOVE_DURATION_SECONDS: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Expression {
    Blink,
    Squint,
    WinkLeft,
    WinkRight,
    Surprise,
    LookUp,
    LookDown,
    Happy,
    Sad,
    Angry,
    Sleepy,
    Curious,
    Love,
}

impl Expression {
    pub(crate) const ALL: [Self; 13] = [
        Self::Blink,
        Self::Squint,
        Self::WinkLeft,
        Self::WinkRight,
        Self::Surprise,
        Self::LookUp,
        Self::LookDown,
        Self::Happy,
        Self::Sad,
        Self::Angry,
        Self::Sleepy,
        Self::Curious,
        Self::Love,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Blink => "Blink",
            Self::Squint => "Squint",
            Self::WinkLeft => "Left wink",
            Self::WinkRight => "Right wink",
            Self::Surprise => "Surprised",
            Self::LookUp => "Look up",
            Self::LookDown => "Look down",
            Self::Happy => "Happy",
            Self::Sad => "Sad",
            Self::Angry => "Angry",
            Self::Sleepy => "Sleepy",
            Self::Curious => "Curious",
            Self::Love => "Love",
        }
    }

    pub(crate) fn compact(self) -> &'static str {
        match self {
            Self::Blink | Self::Sleepy => "-_-",
            Self::Squint => "=_=",
            Self::WinkLeft => "-_o",
            Self::WinkRight => "o_-",
            Self::Surprise => "O_O",
            Self::LookUp | Self::Happy => "^_^",
            Self::LookDown | Self::Sad => "u_u",
            Self::Angry => ">_<",
            Self::Curious => "o_O",
            Self::Love => "<3 <3",
        }
    }

    fn duration(self) -> f64 {
        match self {
            Self::Blink => 0.24,
            Self::Squint => 0.9,
            Self::WinkLeft | Self::WinkRight => 0.4,
            Self::Surprise => 1.2,
            Self::LookUp | Self::LookDown => 0.9,
            Self::Happy | Self::Sad => 1.6,
            Self::Angry => 1.3,
            Self::Sleepy => 2.2,
            Self::Curious => 1.5,
            Self::Love => 1.8,
        }
    }

    fn pose(self, progress: f64) -> FacePose {
        // 平滑进入、短暂停留、平滑退出，避免长表情始终处于变形途中。
        let edge = (progress * 5.0).min((1.0 - progress) * 5.0).clamp(0.0, 1.0);
        let pulse = edge * edge * (3.0 - 2.0 * edge);
        let close = 1.0 - (progress * PI).sin().powi(2);
        let mut pose = FacePose::default();

        match self {
            Self::Blink => {
                pose.eyes[0].scale_y = close;
                pose.eyes[1].scale_y = close;
            }
            Self::Squint => {
                for eye in &mut pose.eyes {
                    eye.scale_y = 1.0 - pulse * 0.55;
                }
            }
            Self::WinkLeft => pose.eyes[0].scale_y = close,
            Self::WinkRight => pose.eyes[1].scale_y = close,
            Self::Surprise => {
                for eye in &mut pose.eyes {
                    eye.scale_x = 1.0 + pulse * 0.08;
                    eye.scale_y = 1.0 + pulse * 0.28;
                }
            }
            Self::LookUp | Self::LookDown => {
                let direction = if self == Self::LookUp { -1.0 } else { 1.0 };
                for eye in &mut pose.eyes {
                    eye.offset_y = direction * pulse * 0.35;
                }
            }
            Self::Happy | Self::Sad | Self::Love => {
                let shape = match self {
                    Self::Happy => EyeShape::Happy,
                    Self::Sad => EyeShape::Sad,
                    _ => EyeShape::Heart,
                };
                for eye in &mut pose.eyes {
                    eye.shape = shape;
                    eye.blend = pulse;
                }
            }
            Self::Angry => {
                for (index, eye) in pose.eyes.iter_mut().enumerate() {
                    eye.scale_y = 1.0 - pulse * 0.35;
                    eye.tilt = if index == 0 {
                        pulse * 0.45
                    } else {
                        -pulse * 0.45
                    };
                }
            }
            Self::Sleepy => {
                for eye in &mut pose.eyes {
                    eye.scale_y = 1.0 - pulse * 0.8;
                    eye.offset_y = pulse * 0.2;
                }
            }
            Self::Curious => {
                pose.eyes[0].scale_y = 1.0 + pulse * 0.25;
                pose.eyes[0].offset_y = -pulse * 0.25;
                pose.eyes[1].scale_y = 1.0 - pulse * 0.5;
                pose.eyes[1].offset_y = pulse * 0.2;
            }
        }
        pose
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum EyeShape {
    #[default]
    Oval,
    Happy,
    Sad,
    Heart,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EyePose {
    pub(crate) scale_x: f64,
    pub(crate) scale_y: f64,
    pub(crate) offset_y: f64,
    pub(crate) tilt: f64,
    pub(crate) shape: EyeShape,
    pub(crate) blend: f64,
}

impl Default for EyePose {
    fn default() -> Self {
        Self {
            scale_x: 1.0,
            scale_y: 1.0,
            offset_y: 0.0,
            tilt: 0.0,
            shape: EyeShape::Oval,
            blend: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FacePose {
    pub(crate) eyes: [EyePose; 2],
    pub(crate) gaze_x: f64,
}

pub(crate) struct AnimationState {
    rng: SimpleRng,
    scheduled_move_window: Option<u64>,
    move_events: Vec<MoveEvent>,
    eye_action: Option<EyeAction>,
    last_expression: Option<Expression>,
    next_eye_action_at: f64,
}

struct MoveEvent {
    start: f64,
    direction: f64,
}

#[derive(Clone, Copy)]
struct EyeAction {
    kind: Expression,
    start: f64,
}

impl AnimationState {
    pub(crate) fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x1234_5678_9abc_def0, |duration| duration.as_nanos() as u64);
        Self::with_seed(seed)
    }

    fn with_seed(seed: u64) -> Self {
        Self {
            rng: SimpleRng { state: seed | 1 },
            scheduled_move_window: None,
            move_events: Vec::with_capacity(4),
            eye_action: None,
            last_expression: None,
            next_eye_action_at: 1.0,
        }
    }

    pub(crate) fn expression(&self) -> Option<Expression> {
        self.eye_action.map(|action| action.kind)
    }

    pub(crate) fn next_expression(&mut self, t: f64) {
        let index = self.last_expression.map_or(0, |last| {
            (Expression::ALL
                .iter()
                .position(|&kind| kind == last)
                .unwrap_or(0)
                + 1)
                % Expression::ALL.len()
        });
        let kind = Expression::ALL[index];
        // 手动切换直接显示动作，暂停时也能预览每一种表情。
        let progress = if matches!(
            kind,
            Expression::Blink | Expression::WinkLeft | Expression::WinkRight
        ) {
            0.5
        } else {
            0.25
        };
        self.eye_action = Some(EyeAction {
            kind,
            start: t - kind.duration() * progress,
        });
        self.last_expression = Some(kind);
    }

    pub(crate) fn sample(&mut self, t: f64) -> FacePose {
        self.schedule_eye_action(t);
        self.schedule_move_events(t);
        self.move_events
            .retain(|event| t < event.start + MOVE_DURATION_SECONDS);

        let mut pose = if let Some(action) = self.eye_action {
            let progress = ((t - action.start) / action.kind.duration()).clamp(0.0, 1.0);
            action.kind.pose(progress)
        } else {
            FacePose::default()
        };
        pose.gaze_x = self
            .move_events
            .iter()
            .filter_map(|event| {
                let progress = (t - event.start) / MOVE_DURATION_SECONDS;
                (0.0..=1.0)
                    .contains(&progress)
                    .then(|| event.direction * (progress * PI).sin())
            })
            .sum();
        pose
    }

    fn schedule_eye_action(&mut self, t: f64) {
        if let Some(action) = self.eye_action {
            if t < action.start + action.kind.duration() {
                return;
            }
            self.eye_action = None;
            self.next_eye_action_at = t + self.rng.next_range(0.8, 2.8);
        }
        if t < self.next_eye_action_at {
            return;
        }

        // 排除上一动作，既保留自然眨眼的高频率，也避免连续重复。
        const WEIGHTS: [u32; 13] = [20, 10, 7, 7, 7, 6, 6, 10, 5, 5, 5, 6, 6];
        let total: u32 = Expression::ALL
            .iter()
            .zip(WEIGHTS)
            .filter(|(kind, _)| Some(**kind) != self.last_expression)
            .map(|(_, weight)| weight)
            .sum();
        let mut roll = self.rng.next_range(0.0, f64::from(total));
        for (kind, weight) in Expression::ALL.into_iter().zip(WEIGHTS) {
            if Some(kind) == self.last_expression {
                continue;
            }
            if roll < f64::from(weight) {
                self.eye_action = Some(EyeAction { kind, start: t });
                self.last_expression = Some(kind);
                break;
            }
            roll -= f64::from(weight);
        }
    }

    fn schedule_move_events(&mut self, t: f64) {
        let window = (t / MOVE_WINDOW_SECONDS).floor() as u64;
        if self.scheduled_move_window == Some(window) {
            return;
        }
        self.scheduled_move_window = Some(window);
        self.move_events.clear();

        let count = if self.rng.next_bool() { 3 } else { 4 };
        let window_start = window as f64 * MOVE_WINDOW_SECONDS;
        let slot = (MOVE_WINDOW_SECONDS - 4.0) / f64::from(count);
        // 每个时间槽只安排一次移动，避免重叠叠加造成突然跳动或偏移过大。
        for index in 0..count {
            let start = window_start
                + 2.0
                + f64::from(index) * slot
                + self.rng.next_range(0.5, slot - MOVE_DURATION_SECONDS - 0.5);
            let direction = if self.rng.next_bool() { 1.0 } else { -1.0 };
            self.move_events.push(MoveEvent { start, direction });
        }
    }
}

struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn next_f64(&mut self) -> f64 {
        // 取高 53 位，确保结果严格小于 1，且不依赖线性同余发生器的低位。
        (self.next_u64() >> 11) as f64 / (1_u64 << 53) as f64
    }

    fn next_range(&mut self, min: f64, max: f64) -> f64 {
        min + (max - min) * self.next_f64()
    }

    fn next_bool(&mut self) -> bool {
        self.next_u64() >> 63 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions_return_to_neutral_at_both_ends() {
        for kind in Expression::ALL {
            for progress in [0.0, 1.0] {
                let pose = kind.pose(progress);
                for eye in pose.eyes {
                    assert!((eye.scale_x - 1.0).abs() < 1e-10, "{kind:?}");
                    assert!((eye.scale_y - 1.0).abs() < 1e-10, "{kind:?}");
                    assert!(eye.offset_y.abs() < 1e-10, "{kind:?}");
                    assert!(eye.tilt.abs() < 1e-10, "{kind:?}");
                    assert!(eye.blend.abs() < 1e-10, "{kind:?}");
                }
            }
        }
    }

    #[test]
    fn blink_closes_both_eyes_and_wink_only_closes_one() {
        assert_eq!(Expression::Blink.pose(0.5).eyes[0].scale_y, 0.0);
        assert_eq!(Expression::Blink.pose(0.5).eyes[1].scale_y, 0.0);
        assert_eq!(Expression::WinkLeft.pose(0.5).eyes[1].scale_y, 1.0);
        assert_eq!(Expression::WinkRight.pose(0.5).eyes[0].scale_y, 1.0);
    }

    #[test]
    fn manual_switch_visits_every_expression_even_when_time_is_frozen() {
        let mut animation = AnimationState::with_seed(7);
        for expected in Expression::ALL {
            animation.next_expression(0.0);
            assert_eq!(animation.expression(), Some(expected));
            let pose = animation.sample(0.0);
            assert_ne!(pose.eyes, FacePose::default().eyes);
        }
        animation.next_expression(0.0);
        assert_eq!(animation.expression(), Some(Expression::Blink));
    }

    #[test]
    fn random_actions_are_varied_and_never_repeat_immediately() {
        let mut animation = AnimationState::with_seed(42);
        let mut seen = Vec::new();
        for index in 0..1000 {
            animation.next_eye_action_at = 0.0;
            animation.eye_action = None;
            let previous = animation.last_expression;
            animation.schedule_eye_action(f64::from(index));
            let current = animation.expression().expect("an action must be scheduled");
            assert_ne!(Some(current), previous);
            if !seen.contains(&current) {
                seen.push(current);
            }
        }
        assert_eq!(seen.len(), Expression::ALL.len());
    }

    #[test]
    fn finished_action_leaves_a_rest_period() {
        let mut animation = AnimationState::with_seed(1);
        animation.next_expression(1.0);
        animation.sample(2.0);
        assert_eq!(animation.expression(), None);
        assert!((2.8..4.8).contains(&animation.next_eye_action_at));
        animation.sample(2.5);
        assert_eq!(animation.expression(), None);
    }

    #[test]
    fn movement_events_stay_in_their_window_without_overlap() {
        let mut animation = AnimationState::with_seed(17);
        let mut counts = [false; 2];
        let mut directions = [false; 2];
        for window in 0..100 {
            let start = f64::from(window) * MOVE_WINDOW_SECONDS;
            animation.schedule_move_events(start);
            counts[animation.move_events.len() - 3] = true;
            let mut previous_end = start;
            for event in &animation.move_events {
                assert!(event.start > previous_end);
                previous_end = event.start + MOVE_DURATION_SECONDS;
                assert!(previous_end < start + MOVE_WINDOW_SECONDS);
                directions[usize::from(event.direction > 0.0)] = true;
            }
        }
        assert_eq!(counts, [true, true]);
        assert_eq!(directions, [true, true]);
    }

    #[test]
    fn sampling_is_stable_and_bounded_over_time_and_after_a_large_jump() {
        let mut animation = AnimationState::with_seed(9);
        for index in 0..10_000 {
            let t = f64::from(index) * 0.05;
            let pose = animation.sample(t);
            assert_eq!(animation.sample(t), pose);
            assert!(pose.gaze_x.abs() <= 1.0);
            for eye in pose.eyes {
                assert!((0.0..=1.3).contains(&eye.scale_y));
                assert!(eye.offset_y.abs() <= 0.35);
            }
        }
        animation.sample(100_000.0);
        assert!(animation.move_events.len() <= 4);
        assert!(
            animation
                .move_events
                .iter()
                .all(|event| event.start >= 100_000.0)
        );
    }

    #[test]
    fn random_fractions_are_in_a_half_open_range() {
        let mut rng = SimpleRng { state: u64::MAX };
        for _ in 0..10_000 {
            assert!((0.0..1.0).contains(&rng.next_f64()));
        }
    }
}
