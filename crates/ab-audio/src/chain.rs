//! A bus's effect chain on kira's audio thread, and its handle on the main thread.
//!
//! Each bus (and the master) runs one kira effect, [`ChainEffect`]: its processors, then the
//! fader, then the meter, so meters, sends and sidechains all see the post-fader signal.
//! The main thread changes it through [`Chain`]: new numbers are applied by the processors
//! in place, a new list of effects is built on the main thread and swapped in whole. The
//! audio thread only ever `try_lock`s, so it never waits on the main thread.

use crate::dsp::biquad::VoiceFilter;
use crate::dsp::meter::{AtomicF32, Levels, Meter};
use crate::dsp::{self, db_to_gain, Context, Processor};
use crate::effect::EffectDesc;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::Frame;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Sample rate assumed until the output reports its own.
const DEFAULT_SAMPLE_RATE: u32 = 48_000;

struct Slot {
    processor: Box<dyn Processor>,
    bypass: bool,
    /// Levels of the bus keying this effect's sidechain.
    key: Option<Arc<Levels>>,
}

/// What the main thread leaves for the audio thread.
#[derive(Default)]
struct Pending {
    /// A new list of processors, to swap in.
    slots: Option<Vec<Slot>>,
    /// The latest settings, one per slot.
    settings: Vec<EffectDesc>,
    /// Processors swapped out, dropped by the main thread (not while mixing).
    retired: Vec<Vec<Slot>>,
}

struct Shared {
    pending: Mutex<Pending>,
    version: AtomicU64,
    levels: Arc<Levels>,
    /// Fader target, linear.
    fader: AtomicF32,
    sample_rate: AtomicU32,
    /// Asks the audio thread to clear the processors' tails.
    flush: AtomicBool,
}

/// Main-thread handle of a chain.
pub(crate) struct Chain {
    shared: Arc<Shared>,
    /// What was last sent.
    effects: Vec<EffectDesc>,
    /// Sidechain buses of `effects`, as resolved.
    keys: Vec<Option<String>>,
    built_rate: u32,
}

impl Chain {
    /// A chain with no effects at unity gain, and the effect to add to a kira track.
    pub fn new() -> (Self, ChainBuilder) {
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending {
                retired: Vec::with_capacity(4),
                ..Pending::default()
            }),
            version: AtomicU64::new(0),
            levels: Arc::new(Levels::default()),
            fader: AtomicF32::new(1.0),
            sample_rate: AtomicU32::new(0),
            flush: AtomicBool::new(false),
        });
        (
            Self {
                shared: shared.clone(),
                effects: Vec::new(),
                keys: Vec::new(),
                built_rate: DEFAULT_SAMPLE_RATE,
            },
            ChainBuilder { shared },
        )
    }

    pub fn levels(&self) -> &Arc<Levels> {
        &self.shared.levels
    }

    /// The output's sample rate once known.
    fn sample_rate(&self) -> u32 {
        match self.shared.sample_rate.load(Ordering::Relaxed) {
            0 => DEFAULT_SAMPLE_RATE,
            rate => rate,
        }
    }

    /// Clears what the effects still ring with (reverb and delay tails).
    pub fn flush(&self) {
        self.shared.flush.store(true, Ordering::Relaxed);
    }

    /// Fader level in dB (glides over one block).
    pub fn set_fader_db(&self, db: f32) {
        self.shared.fader.store(db_to_gain(db));
    }

    /// Applies `effects`; `key` finds the levels of a sidechain bus by name.
    pub fn set_effects(
        &mut self,
        effects: &[EffectDesc],
        key: &dyn Fn(&str) -> Option<Arc<Levels>>,
    ) {
        let rate = self.sample_rate();
        let keys: Vec<Option<String>> = effects
            .iter()
            .map(|e| e.kind.sidechain().map(str::to_owned))
            .collect();
        let rebuild = rate != self.built_rate
            || effects.len() != self.effects.len()
            || keys != self.keys
            || effects
                .iter()
                .zip(&self.effects)
                .any(|(new, old)| !new.kind.same_shape(&old.kind));
        if !rebuild && effects == self.effects.as_slice() {
            return;
        }
        let slots = rebuild.then(|| {
            effects
                .iter()
                .map(|e| Slot {
                    processor: dsp::build(&e.kind, rate as f32),
                    bypass: e.bypass,
                    key: e.kind.sidechain().and_then(key),
                })
                .collect::<Vec<_>>()
        });
        let retired = {
            let mut pending = self
                .shared
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slots.is_some() {
                pending.slots = slots;
            }
            pending.settings.clear();
            pending.settings.extend_from_slice(effects);
            std::mem::take(&mut pending.retired)
        };
        drop(retired);
        self.shared.version.fetch_add(1, Ordering::Release);
        self.effects = effects.to_vec();
        self.keys = keys;
        self.built_rate = rate;
    }

    /// Rebuilds the processors if the output turned out to run at another rate than they
    /// were built for (once, after the device opens).
    pub fn follow_sample_rate(&mut self, key: &dyn Fn(&str) -> Option<Arc<Levels>>) {
        if self.sample_rate() != self.built_rate {
            let effects = std::mem::take(&mut self.effects);
            self.set_effects(&effects, key);
        }
    }
}

/// Adds a chain to a kira track (see [`Chain::new`]).
pub(crate) struct ChainBuilder {
    shared: Arc<Shared>,
}

impl EffectBuilder for ChainBuilder {
    type Handle = ();

    fn build(self) -> (Box<dyn Effect>, ()) {
        let rate = DEFAULT_SAMPLE_RATE as f32;
        (
            Box::new(ChainEffect {
                shared: self.shared,
                slots: Vec::new(),
                seen: 0,
                gain: 1.0,
                meter: Meter::new(rate),
                sample_rate: rate,
            }),
            (),
        )
    }
}

/// The audio-thread side of a [`Chain`].
struct ChainEffect {
    shared: Arc<Shared>,
    slots: Vec<Slot>,
    seen: u64,
    /// Fader gain reached at the end of the last block.
    gain: f32,
    meter: Meter,
    sample_rate: f32,
}

impl ChainEffect {
    /// Picks up what the main thread changed, if it can without waiting.
    fn sync(&mut self) {
        let version = self.shared.version.load(Ordering::Acquire);
        if version == self.seen {
            return;
        }
        let Ok(mut pending) = self.shared.pending.try_lock() else {
            return;
        };
        if let Some(slots) = pending.slots.take() {
            let old = std::mem::replace(&mut self.slots, slots);
            // Within the reserved capacity: no allocation here.
            if pending.retired.len() < pending.retired.capacity() {
                pending.retired.push(old);
            } else {
                drop(old);
            }
        }
        for (slot, settings) in self.slots.iter_mut().zip(&pending.settings) {
            slot.processor.configure(&settings.kind, self.sample_rate);
            slot.bypass = settings.bypass;
        }
        self.seen = version;
    }
}

impl Effect for ChainEffect {
    fn init(&mut self, sample_rate: u32, _internal_buffer_size: usize) {
        self.on_change_sample_rate(sample_rate);
    }

    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
        self.meter = Meter::new(self.sample_rate);
        self.shared
            .sample_rate
            .store(sample_rate, Ordering::Relaxed);
        self.shared
            .levels
            .sample_rate
            .store(sample_rate, Ordering::Relaxed);
    }

    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info<'_>) {
        self.sync();
        if self.shared.flush.swap(false, Ordering::Relaxed) {
            for slot in &mut self.slots {
                slot.processor.reset();
            }
        }
        let levels = &self.shared.levels;
        for slot in &mut self.slots {
            if slot.bypass {
                continue;
            }
            let context = Context {
                key: slot.key.as_ref().map(|k| k.key()),
            };
            slot.processor.process(input, &context);
            levels.report_reduction(slot.processor.gain_reduction_db());
        }
        // The fader glides over the block: no zipper noise from moving it.
        let target = self.shared.fader.load();
        let target = if target.is_finite() { target } else { 0.0 };
        let step = (target - self.gain) / input.len().max(1) as f32;
        for frame in input.iter_mut() {
            self.gain += step;
            *frame *= self.gain;
        }
        self.gain = target;
        self.meter.measure(input, levels);
    }
}

/// Per-voice processing on a 3D sound's track: the occlusion low-pass.
pub(crate) struct VoiceEffectBuilder {
    pub cutoff: Arc<AtomicF32>,
}

impl EffectBuilder for VoiceEffectBuilder {
    type Handle = ();

    fn build(self) -> (Box<dyn Effect>, ()) {
        (
            Box::new(VoiceEffect {
                cutoff: self.cutoff,
                filter: VoiceFilter::default(),
                sample_rate: DEFAULT_SAMPLE_RATE as f32,
            }),
            (),
        )
    }
}

struct VoiceEffect {
    cutoff: Arc<AtomicF32>,
    filter: VoiceFilter,
    sample_rate: f32,
}

impl Effect for VoiceEffect {
    fn init(&mut self, sample_rate: u32, _: usize) {
        self.sample_rate = sample_rate as f32;
    }

    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
    }

    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info<'_>) {
        self.filter.set_cutoff(self.cutoff.load(), self.sample_rate);
        self.filter.process(input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::tests::{peak, sine};
    use crate::effect::{EffectKind, UtilitySettings};

    fn no_keys(_: &str) -> Option<Arc<Levels>> {
        None
    }

    fn process(effect: &mut dyn Effect, frames: &mut [Frame]) {
        let info = kira::info::MockInfoBuilder::new().build();
        for block in frames.chunks_mut(128) {
            effect.process(block, 1.0 / 48_000.0, &info);
        }
    }

    fn gain(db: f32) -> EffectDesc {
        EffectDesc::new(EffectKind::Utility(UtilitySettings {
            gain_db: db,
            ..Default::default()
        }))
    }

    #[test]
    fn settings_reach_the_audio_thread_in_place_or_as_a_new_chain() {
        let (mut chain, builder) = Chain::new();
        let (mut effect, ()) = builder.build();
        effect.init(48_000, 128);

        chain.set_effects(&[gain(-6.0206)], &no_keys);
        let mut signal = sine(100.0, 1.0, 0.01);
        process(effect.as_mut(), &mut signal);
        assert!((peak(&signal) - 0.5).abs() < 1e-3);

        // Numbers only: same processors.
        chain.set_effects(&[gain(-12.0412)], &no_keys);
        let mut signal = sine(100.0, 1.0, 0.01);
        process(effect.as_mut(), &mut signal);
        assert!((peak(&signal) - 0.25).abs() < 1e-3);

        // Bypassed.
        let mut bypassed = gain(-12.0);
        bypassed.bypass = true;
        chain.set_effects(&[bypassed], &no_keys);
        let mut signal = sine(100.0, 1.0, 0.01);
        process(effect.as_mut(), &mut signal);
        assert!((peak(&signal) - 1.0).abs() < 1e-3);

        // A new list.
        chain.set_effects(&[gain(-6.0206), gain(-6.0206)], &no_keys);
        let mut signal = sine(100.0, 1.0, 0.01);
        process(effect.as_mut(), &mut signal);
        assert!((peak(&signal) - 0.25).abs() < 1e-3);
    }

    #[test]
    fn the_fader_glides_and_the_meter_is_after_it() {
        let (chain, builder) = Chain::new();
        let (mut effect, ()) = builder.build();
        effect.init(48_000, 128);
        chain.set_fader_db(-6.0206);
        let mut signal = vec![Frame::from_mono(1.0); 256];
        process(effect.as_mut(), &mut signal);
        assert!(signal[0].left > 0.99, "starts from where it was");
        assert!(
            (signal[127].left - 0.5).abs() < 1e-3,
            "reaches it by the block's end"
        );
        assert!((signal[255].left - 0.5).abs() < 1e-3);
        let reading = chain.levels().read();
        assert!(
            (reading.peak[0] - 1.0).abs() < 0.01,
            "the first sample of the glide"
        );
        assert!((chain.levels().key() - 0.5).abs() < 1e-3, "post-fader key");
    }
}
