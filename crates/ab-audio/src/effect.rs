//! Effect settings: what a bus's effect chain holds in `audio.toml`, and what snapshots
//! blend. The processing itself is in [`crate::dsp`].

use serde::{Deserialize, Serialize};

/// One effect in a chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectDesc {
    /// Passed through untouched (the processor keeps its state).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypass: bool,
    #[serde(flatten)]
    pub kind: EffectKind,
}

impl EffectDesc {
    pub fn new(kind: EffectKind) -> Self {
        Self {
            bypass: false,
            kind,
        }
    }

    /// `self` moved `t` (0..=1) of the way to `other`; see [`EffectKind::blend`].
    pub fn blend(&self, other: &Self, t: f32) -> Self {
        Self {
            bypass: if t < 0.5 { self.bypass } else { other.bypass },
            kind: self.kind.blend(&other.kind, t),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectKind {
    /// Parametric equalizer: up to [`EqSettings::MAX_BANDS`] bands.
    Eq(EqSettings),
    /// One filter band (low-pass for muffling, high-pass for radios).
    Filter(EqBand),
    Compressor(CompressorSettings),
    /// Brick-wall peak limiter with look-ahead, the last thing on the master.
    Limiter(LimiterSettings),
    Reverb(ReverbSettings),
    Delay(DelaySettings),
    Saturation(SaturationSettings),
    /// Gain, pan and stereo width.
    Utility(UtilitySettings),
}

/// The effect kinds in menu order, with their defaults.
pub const EFFECT_KINDS: &[&str] = &[
    "eq",
    "filter",
    "compressor",
    "limiter",
    "reverb",
    "delay",
    "saturation",
    "utility",
];

impl EffectKind {
    /// The default effect of a kind named as in `audio.toml` (`"eq"`, `"reverb"`...).
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "eq" => Self::Eq(EqSettings::default()),
            "filter" => Self::Filter(EqBand {
                shape: FilterShape::HighCut,
                freq: 2_000.0,
                ..EqBand::default()
            }),
            "compressor" => Self::Compressor(CompressorSettings::default()),
            "limiter" => Self::Limiter(LimiterSettings::default()),
            "reverb" => Self::Reverb(ReverbSettings::default()),
            "delay" => Self::Delay(DelaySettings::default()),
            "saturation" => Self::Saturation(SaturationSettings::default()),
            "utility" => Self::Utility(UtilitySettings::default()),
            _ => return None,
        })
    }

    /// The name used in `audio.toml`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Eq(_) => "eq",
            Self::Filter(_) => "filter",
            Self::Compressor(_) => "compressor",
            Self::Limiter(_) => "limiter",
            Self::Reverb(_) => "reverb",
            Self::Delay(_) => "delay",
            Self::Saturation(_) => "saturation",
            Self::Utility(_) => "utility",
        }
    }

    /// Whether `other` can be applied to this effect's processor without rebuilding it:
    /// same kind, same number of EQ bands, same sidechain.
    pub fn same_shape(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Eq(a), Self::Eq(b)) => a.bands.len() == b.bands.len(),
            (Self::Compressor(a), Self::Compressor(b)) => a.sidechain == b.sidechain,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }

    /// The bus whose level drives this effect (a compressor ducking under dialogue).
    pub fn sidechain(&self) -> Option<&str> {
        match self {
            Self::Compressor(c) => c.sidechain.as_deref().filter(|s| !s.is_empty()),
            _ => None,
        }
    }

    /// `self` moved `t` (0..=1) of the way to `other`: numbers interpolate, choices switch
    /// half way. Effects of a different shape switch half way as a whole.
    pub fn blend(&self, other: &Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        if !self.same_shape(other) {
            return if t < 0.5 { self.clone() } else { other.clone() };
        }
        match (self, other) {
            (Self::Eq(a), Self::Eq(b)) => Self::Eq(EqSettings {
                bands: a
                    .bands
                    .iter()
                    .zip(&b.bands)
                    .map(|(a, b)| a.blend(b, t))
                    .collect(),
                output_db: lerp(a.output_db, b.output_db, t),
            }),
            (Self::Filter(a), Self::Filter(b)) => Self::Filter(a.blend(b, t)),
            (Self::Compressor(a), Self::Compressor(b)) => Self::Compressor(CompressorSettings {
                threshold_db: lerp(a.threshold_db, b.threshold_db, t),
                ratio: lerp(a.ratio, b.ratio, t),
                attack_ms: lerp(a.attack_ms, b.attack_ms, t),
                release_ms: lerp(a.release_ms, b.release_ms, t),
                knee_db: lerp(a.knee_db, b.knee_db, t),
                makeup_db: lerp(a.makeup_db, b.makeup_db, t),
                mix: lerp(a.mix, b.mix, t),
                sidechain: a.sidechain.clone(),
            }),
            (Self::Limiter(a), Self::Limiter(b)) => Self::Limiter(LimiterSettings {
                input_db: lerp(a.input_db, b.input_db, t),
                ceiling_db: lerp(a.ceiling_db, b.ceiling_db, t),
                release_ms: lerp(a.release_ms, b.release_ms, t),
                lookahead_ms: lerp(a.lookahead_ms, b.lookahead_ms, t),
            }),
            (Self::Reverb(a), Self::Reverb(b)) => Self::Reverb(ReverbSettings {
                room_size: lerp(a.room_size, b.room_size, t),
                damping: lerp(a.damping, b.damping, t),
                width: lerp(a.width, b.width, t),
                pre_delay_ms: lerp(a.pre_delay_ms, b.pre_delay_ms, t),
                low_cut_hz: lerp_log(a.low_cut_hz, b.low_cut_hz, t),
                high_cut_hz: lerp_log(a.high_cut_hz, b.high_cut_hz, t),
                mix: lerp(a.mix, b.mix, t),
            }),
            (Self::Delay(a), Self::Delay(b)) => Self::Delay(DelaySettings {
                time_ms: lerp(a.time_ms, b.time_ms, t),
                feedback: lerp(a.feedback, b.feedback, t),
                mix: lerp(a.mix, b.mix, t),
                ping_pong: if t < 0.5 { a.ping_pong } else { b.ping_pong },
                high_cut_hz: lerp_log(a.high_cut_hz, b.high_cut_hz, t),
            }),
            (Self::Saturation(a), Self::Saturation(b)) => Self::Saturation(SaturationSettings {
                drive_db: lerp(a.drive_db, b.drive_db, t),
                mix: lerp(a.mix, b.mix, t),
                output_db: lerp(a.output_db, b.output_db, t),
            }),
            (Self::Utility(a), Self::Utility(b)) => Self::Utility(UtilitySettings {
                gain_db: lerp(a.gain_db, b.gain_db, t),
                pan: lerp(a.pan, b.pan, t),
                width: lerp(a.width, b.width, t),
            }),
            _ => unreachable!("same_shape checked the kinds"),
        }
    }
}

pub(crate) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Interpolates frequencies on a log scale (an octave is an octave anywhere).
pub(crate) fn lerp_log(a: f32, b: f32, t: f32) -> f32 {
    if a > 0.0 && b > 0.0 {
        (a.ln() + (b.ln() - a.ln()) * t).exp()
    } else {
        lerp(a, b, t)
    }
}

/// Response shape of a filter band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterShape {
    /// Bell: boosts or cuts around `freq`.
    #[default]
    Peak,
    LowShelf,
    HighShelf,
    /// Removes what is below `freq` (high-pass).
    LowCut,
    /// Removes what is above `freq` (low-pass).
    HighCut,
    BandPass,
    Notch,
}

impl FilterShape {
    pub const ALL: [FilterShape; 7] = [
        Self::Peak,
        Self::LowShelf,
        Self::HighShelf,
        Self::LowCut,
        Self::HighCut,
        Self::BandPass,
        Self::Notch,
    ];

    /// Whether `gain_db` matters (bells and shelves).
    pub fn has_gain(self) -> bool {
        matches!(self, Self::Peak | Self::LowShelf | Self::HighShelf)
    }

    /// Whether `slope` matters (cut filters).
    pub fn has_slope(self) -> bool {
        matches!(self, Self::LowCut | Self::HighCut)
    }
}

/// One band of an equalizer or filter.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqBand {
    pub shape: FilterShape,
    /// Centre or corner frequency, Hz.
    pub freq: f32,
    /// Boost or cut of bells and shelves, dB.
    pub gain_db: f32,
    /// Width (bells) or resonance (cuts); 0.707 is flat.
    pub q: f32,
    /// Steepness of cut filters: 12, 24, 36 or 48 dB per octave.
    pub slope: u8,
    pub enabled: bool,
}

impl Default for EqBand {
    fn default() -> Self {
        Self {
            shape: FilterShape::Peak,
            freq: 1_000.0,
            gain_db: 0.0,
            q: std::f32::consts::FRAC_1_SQRT_2,
            slope: 12,
            enabled: true,
        }
    }
}

impl EqBand {
    /// Lowest and highest frequency a band can take.
    pub const FREQ_RANGE: (f32, f32) = (20.0, 20_000.0);
    pub const GAIN_RANGE: (f32, f32) = (-24.0, 24.0);
    pub const Q_RANGE: (f32, f32) = (0.1, 18.0);

    fn blend(&self, other: &Self, t: f32) -> Self {
        let half = t >= 0.5;
        Self {
            shape: if half { other.shape } else { self.shape },
            freq: lerp_log(self.freq, other.freq, t),
            gain_db: lerp(self.gain_db, other.gain_db, t),
            q: lerp_log(self.q, other.q, t),
            slope: if half { other.slope } else { self.slope },
            enabled: if half { other.enabled } else { self.enabled },
        }
    }

    /// Biquad stages the slope takes (12 dB/oct each).
    pub fn stages(&self) -> usize {
        if self.shape.has_slope() {
            (usize::from(self.slope) / 12).clamp(1, 4)
        } else {
            1
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqSettings {
    pub bands: Vec<EqBand>,
    /// Gain after the bands, dB.
    pub output_db: f32,
}

impl EqSettings {
    pub const MAX_BANDS: usize = 8;
}

impl Default for EqSettings {
    /// A low cut, three bells and a high shelf, all flat.
    fn default() -> Self {
        let band = |shape, freq| EqBand {
            shape,
            freq,
            ..EqBand::default()
        };
        Self {
            bands: vec![
                EqBand {
                    enabled: false,
                    ..band(FilterShape::LowCut, 40.0)
                },
                band(FilterShape::LowShelf, 120.0),
                band(FilterShape::Peak, 500.0),
                band(FilterShape::Peak, 2_500.0),
                band(FilterShape::HighShelf, 8_000.0),
            ],
            output_db: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressorSettings {
    /// Level above which the signal is turned down, dB.
    pub threshold_db: f32,
    /// 4 = every 4 dB over the threshold come out as 1.
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// Width of the soft knee around the threshold, dB (0 = hard).
    pub knee_db: f32,
    /// Gain after compression, dB.
    pub makeup_db: f32,
    /// Compressed share of the output (parallel compression), 0..=1.
    pub mix: f32,
    /// A bus whose level drives the compressor instead of its own input: music ducks
    /// under dialogue with `sidechain = "Voice"` on the music bus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidechain: Option<String>,
}

impl Default for CompressorSettings {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            knee_db: 6.0,
            makeup_db: 0.0,
            mix: 1.0,
            sidechain: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LimiterSettings {
    /// Gain before limiting, dB (pushes the level up to the ceiling).
    pub input_db: f32,
    /// No peak leaves above this, dB.
    pub ceiling_db: f32,
    pub release_ms: f32,
    /// How early peaks are seen coming, ms (0.5..=10): longer is cleaner, adds latency.
    pub lookahead_ms: f32,
}

impl LimiterSettings {
    pub const MAX_LOOKAHEAD_MS: f32 = 10.0;
}

impl Default for LimiterSettings {
    fn default() -> Self {
        Self {
            input_db: 0.0,
            ceiling_db: -0.3,
            release_ms: 80.0,
            lookahead_ms: 3.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReverbSettings {
    /// 0 (closet) ..= 1 (cathedral).
    pub room_size: f32,
    /// How fast highs die out, 0..=1.
    pub damping: f32,
    /// Stereo width of the tail, 0..=1.
    pub width: f32,
    /// Silence before the tail starts, ms.
    pub pre_delay_ms: f32,
    /// The tail without lows below this, Hz (keeps it from booming).
    pub low_cut_hz: f32,
    /// The tail without highs above this, Hz.
    pub high_cut_hz: f32,
    /// Wet share of the output, 0..=1 (1 on a return bus).
    pub mix: f32,
}

impl ReverbSettings {
    pub const MAX_PRE_DELAY_MS: f32 = 250.0;
}

impl Default for ReverbSettings {
    fn default() -> Self {
        Self {
            room_size: 0.6,
            damping: 0.5,
            width: 1.0,
            pre_delay_ms: 20.0,
            low_cut_hz: 150.0,
            high_cut_hz: 9_000.0,
            mix: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DelaySettings {
    pub time_ms: f32,
    /// Share of each echo fed back into the next, 0..0.95.
    pub feedback: f32,
    /// Wet share of the output, 0..=1.
    pub mix: f32,
    /// Echoes alternate between left and right.
    pub ping_pong: bool,
    /// Each echo darker than the last above this, Hz.
    pub high_cut_hz: f32,
}

impl DelaySettings {
    pub const MAX_TIME_MS: f32 = 2_000.0;
}

impl Default for DelaySettings {
    fn default() -> Self {
        Self {
            time_ms: 350.0,
            feedback: 0.35,
            mix: 0.3,
            ping_pong: false,
            high_cut_hz: 6_000.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaturationSettings {
    /// Gain into the soft clipper, dB.
    pub drive_db: f32,
    pub mix: f32,
    pub output_db: f32,
}

impl Default for SaturationSettings {
    fn default() -> Self {
        Self {
            drive_db: 6.0,
            mix: 1.0,
            output_db: -3.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UtilitySettings {
    pub gain_db: f32,
    /// -1 (left) ..= 1 (right).
    pub pan: f32,
    /// 0 (mono) ..= 2 (extra wide); 1 unchanged.
    pub width: f32,
}

impl Default for UtilitySettings {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            pan: 0.0,
            width: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_read_and_write_as_toml_tables() {
        #[derive(Serialize, Deserialize)]
        struct Chain {
            effect: Vec<EffectDesc>,
        }
        let text = r#"
            [[effect]]
            type = "compressor"
            threshold_db = -24.0
            sidechain = "Voice"

            [[effect]]
            type = "eq"
            bypass = true
            bands = [{ shape = "low_cut", freq = 80.0, slope = 24 }]
        "#;
        let chain: Chain = toml::from_str(text).unwrap();
        let EffectKind::Compressor(c) = &chain.effect[0].kind else {
            panic!("a compressor");
        };
        assert_eq!(c.threshold_db, -24.0);
        assert_eq!(c.ratio, 4.0, "unset numbers keep their defaults");
        assert_eq!(chain.effect[0].kind.sidechain(), Some("Voice"));
        assert!(chain.effect[1].bypass);
        let EffectKind::Eq(eq) = &chain.effect[1].kind else {
            panic!("an eq");
        };
        assert_eq!(eq.bands[0].stages(), 2);

        let again: Chain = toml::from_str(&toml::to_string(&chain).unwrap()).unwrap();
        assert_eq!(again.effect, chain.effect);
    }

    #[test]
    fn every_kind_has_a_default_by_name() {
        for name in EFFECT_KINDS {
            assert_eq!(EffectKind::from_name(name).unwrap().name(), *name);
        }
    }

    #[test]
    fn blending_interpolates_numbers_and_switches_shapes_half_way() {
        let a = EffectKind::Filter(EqBand {
            shape: FilterShape::HighCut,
            freq: 200.0,
            ..EqBand::default()
        });
        let b = EffectKind::Filter(EqBand {
            shape: FilterShape::HighCut,
            freq: 20_000.0,
            ..EqBand::default()
        });
        let EffectKind::Filter(mid) = a.blend(&b, 0.5) else {
            unreachable!()
        };
        assert!((mid.freq - 2_000.0).abs() < 1.0, "log-scale midpoint");

        let reverb = EffectKind::from_name("reverb").unwrap();
        assert_eq!(a.blend(&reverb, 0.4), a);
        assert_eq!(a.blend(&reverb, 0.6), reverb);
    }
}
