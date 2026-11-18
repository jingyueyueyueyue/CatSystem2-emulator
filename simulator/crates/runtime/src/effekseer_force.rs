//! Absolute world-space controllers from PE e2430..e2751. Independent code;
//! target-specific epsilon and load-time scaling differ from the public101 text.
use super::Error;

#[derive(Debug, Clone)]
pub(super) enum Absolute {
    None,
    Gravity([f32; 3]),
    Attraction {
        force: f32,
        control: f32,
        min_range: f32,
        max_range: f32,
    },
}
impl Absolute {
    pub fn magnify(&mut self, k: f32) {
        match self {
            Self::None => {}
            Self::Gravity(g) => g.iter_mut().for_each(|v| *v *= k),
            Self::Attraction {
                force,
                control,
                min_range,
                max_range,
            } => {
                // PE dcadf..dcb36 multiplies all four stored fields,
                // including control. Do not substitute dimensional scaling.
                for v in [force, control, min_range, max_range] {
                    *v *= k;
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ForceState {
    previous_position: [f32; 3],
    revision: [f32; 3],
    velocity: [f32; 3],
}
impl ForceState {
    pub fn new(parent_position: [f32; 3]) -> Self {
        Self {
            previous_position: parent_position,
            revision: [0.0; 3],
            velocity: [0.0; 3],
        }
    }
    pub fn sample(
        &mut self,
        controller: &Absolute,
        current: [f32; 3],
        target: [f32; 3],
        age: f32,
        delta: f32,
    ) -> Result<[f32; 3], Error> {
        // e2136..e2182 stores currentPosition and its difference before
        // the controller. Keep the actual subtract/add order used downstream.
        let motion: [f32; 3] = std::array::from_fn(|i| current[i] - self.previous_position[i]);
        self.previous_position = current;
        match controller {
            Absolute::None => {}
            Absolute::Gravity(g) => {
                self.revision = std::array::from_fn(|i| g[i] * age * age * 0.5);
            }
            Absolute::Attraction {
                force,
                control,
                min_range,
                max_range,
            } => {
                let difference: [f32; 3] = std::array::from_fn(|i| {
                    target[i] - (current[i] - motion[i] + self.revision[i])
                });
                let distance = length(difference);
                if !distance.is_finite() {
                    return Err(Error::Host("nonfinite SKFE attraction distance"));
                }
                if distance > 0.0 && delta > 0.0 {
                    let direction = difference.map(|v| v * (1.0 / distance));
                    let mut acceleration = *force;
                    if *min_range > 0.0 || *max_range > 0.0 {
                        if distance >= *max_range {
                            acceleration = 0.0;
                        } else if distance > *min_range {
                            acceleration *=
                                1.0 - (distance - *min_range) / (*max_range - *min_range);
                        }
                    }
                    let accelerated = std::array::from_fn::<_, 3, _>(|i| {
                        direction[i] * acceleration * delta + self.velocity[i]
                    });
                    // PE e2648..e2659: actual epsilon is added to the norm
                    // before both normalization and rescaling (not a clamp).
                    let speed = length(accelerated) + f32::from_bits(0x38d1b717);
                    let reciprocal = 1.0 / speed;
                    self.velocity = std::array::from_fn(|i| {
                        (accelerated[i] * reciprocal * (1.0 - *control) + direction[i] * *control)
                            * speed
                    });
                    for i in 0..3 {
                        self.revision[i] += self.velocity[i] * delta;
                    }
                }
            }
        }
        if self
            .revision
            .into_iter()
            .chain(self.velocity)
            .all(f32::is_finite)
        {
            Ok(self.revision)
        } else {
            Err(Error::Host("nonfinite SKFE absolute controller"))
        }
    }
}
fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}
