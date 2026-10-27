//! Widgets of the audio tools: level meters, faders, the equalizer's curve with its
//! spectrum, the compressor's curve, distance rolloff and the clip waveform.

use crate::i18n::t;
use crate::theme;
use ab_engine::audio::clip::ClipEdit;
use ab_engine::audio::dsp::biquad::band_response_db;
use ab_engine::audio::dsp::dynamics::compressor_curve;
use ab_engine::audio::dsp::{db_to_gain, gain_to_db};
use ab_engine::audio::effect::{
    CompressorSettings, DelaySettings, EffectDesc, EffectKind, EqBand, EqSettings, FilterShape,
    LimiterSettings, ReverbSettings, SaturationSettings, UtilitySettings,
};
use ab_engine::audio::{Attenuation, MeterReading, Rolloff};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};

/// Lowest level the meters and faders show, dB.
pub const FLOOR_DB: f32 = -60.0;
/// Fader range, dB.
pub const FADER_MAX_DB: f32 = 12.0;

// =============================================================================================
// Scales
// =============================================================================================

/// Fader position (0 bottom .. 1 top) of a level: 0 dB sits at about three quarters, like a
/// mixing desk, with the bottom tenth for -60 dB down to silence.
pub fn fader_position(db: f32) -> f32 {
    if !db.is_finite() || db <= -80.0 {
        0.0
    } else if db < FLOOR_DB {
        (db + 80.0) / 20.0 * 0.1
    } else {
        let x = ((db - FLOOR_DB) / (FADER_MAX_DB - FLOOR_DB)).clamp(0.0, 1.0);
        0.1 + 0.9 * x.powf(1.5)
    }
}

/// The level at a fader position (inverse of [`fader_position`]).
pub fn fader_db(position: f32) -> f32 {
    let p = position.clamp(0.0, 1.0);
    if p <= 0.0 {
        ab_engine::audio::dsp::SILENCE_DB
    } else if p < 0.1 {
        p / 0.1 * 20.0 - 80.0
    } else {
        ((p - 0.1) / 0.9).powf(1.0 / 1.5) * (FADER_MAX_DB - FLOOR_DB) + FLOOR_DB
    }
}

/// Meter position (0..1) of a linear level, on a dB scale from [`FLOOR_DB`] to +6.
fn meter_position(level: f32) -> f32 {
    ((gain_to_db(level) - FLOOR_DB) / (6.0 - FLOOR_DB)).clamp(0.0, 1.0)
}

/// A level in dB for display ("-inf" for silence).
pub fn format_db(db: f32) -> String {
    if db <= -79.95 || !db.is_finite() {
        "-∞".to_owned()
    } else {
        format!("{db:+.1}")
    }
}

/// X of `freq` Hz in `rect` on a log scale from 20 Hz to 20 kHz.
fn freq_x(rect: Rect, freq: f32) -> f32 {
    let t = (freq.max(1.0).log10() - 20f32.log10()) / (20_000f32.log10() - 20f32.log10());
    rect.left() + t * rect.width()
}

fn x_freq(rect: Rect, x: f32) -> f32 {
    let t = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    10f32.powf(20f32.log10() + t * (20_000f32.log10() - 20f32.log10()))
}

fn format_freq(freq: f32) -> String {
    if freq >= 1000.0 {
        format!("{:.1}k", freq / 1000.0)
    } else {
        format!("{freq:.0}")
    }
}

// =============================================================================================
// Meters and faders
// =============================================================================================

/// What a meter keeps between frames: falling bars and held peaks.
#[derive(Debug, Clone, Copy, Default)]
pub struct MeterState {
    peak: [f32; 2],
    rms: [f32; 2],
    hold: [f32; 2],
    hold_time: [f64; 2],
    reduction_db: f32,
    clipped_at: Option<f64>,
}

impl MeterState {
    /// Takes a new reading: bars rise at once and fall ~20 dB/s; peaks hold 1.5 s.
    pub fn update(&mut self, reading: &MeterReading, now: f64, delta: f32) {
        let fall = db_to_gain(-20.0 * delta.clamp(0.0, 0.2));
        self.rms = reading.rms;
        for side in 0..2 {
            self.peak[side] = (self.peak[side] * fall).max(reading.peak[side]);
            if reading.peak[side] >= self.hold[side] || now - self.hold_time[side] > 1.5 {
                self.hold[side] = reading.peak[side];
                self.hold_time[side] = now;
            }
        }
        self.reduction_db = (self.reduction_db - 30.0 * delta).max(reading.reduction_db);
        if reading.clipped {
            self.clipped_at = Some(now);
        }
    }

    /// Whether the clip light is on (for 3 s after clipping, or until clicked).
    pub fn clipping(&self, now: f64) -> bool {
        self.clipped_at.is_some_and(|at| now - at < 3.0)
    }

    pub fn reset_clip(&mut self) {
        self.clipped_at = None;
    }

    /// Gain reduction of the bus's dynamics, falling back slowly, dB.
    pub fn reduction_db(&self) -> f32 {
        self.reduction_db
    }

    /// The held peak of the louder side, dB.
    pub fn peak_db(&self) -> f32 {
        gain_to_db(self.hold[0].max(self.hold[1]))
    }
}

fn meter_color(position: f32) -> Color32 {
    let db = FLOOR_DB + position * (6.0 - FLOOR_DB);
    if db > -0.1 {
        theme::ERROR
    } else if db > -9.0 {
        theme::WARN
    } else {
        theme::PLAY
    }
}

/// Two vertical bars (left, right) with RMS, peak, held peak and a gain-reduction strip.
pub fn meter(ui: &mut egui::Ui, state: &MeterState, size: Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let rms = state.rms;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, theme::BG_0);
    let gr_width = 4.0;
    let bars = Rect::from_min_max(
        rect.min,
        Pos2::new(rect.right() - gr_width - 2.0, rect.bottom()),
    );
    let bar_width = (bars.width() - 2.0) / 2.0;
    for (side, rms) in rms.into_iter().enumerate() {
        let left = bars.left() + side as f32 * (bar_width + 2.0);
        let column = Rect::from_min_size(
            Pos2::new(left, bars.top()),
            Vec2::new(bar_width, bars.height()),
        );
        let y = |p: f32| column.bottom() - p * column.height();
        // The peak bar in segments coloured by level, the RMS brighter inside it.
        let peak = meter_position(state.peak[side]);
        let steps = 24;
        for i in 0..steps {
            let from = i as f32 / steps as f32;
            if from >= peak {
                break;
            }
            let to = ((i + 1) as f32 / steps as f32).min(peak);
            let segment = Rect::from_min_max(
                Pos2::new(column.left(), y(to)),
                Pos2::new(column.right(), y(from) - 0.5),
            );
            painter.rect_filled(segment, 0.0, meter_color(from).gamma_multiply(0.55));
        }
        let rms_p = meter_position(rms);
        if rms_p > 0.0 {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(column.left(), y(rms_p)),
                    column.left_bottom() + Vec2::new(bar_width, 0.0),
                ),
                0.0,
                meter_color(rms_p),
            );
        }
        let hold = meter_position(state.hold[side]);
        if hold > 0.0 {
            painter.line_segment(
                [
                    Pos2::new(column.left(), y(hold)),
                    Pos2::new(column.right(), y(hold)),
                ],
                Stroke::new(1.5, meter_color(hold)),
            );
        }
    }
    // 0 dB line.
    let zero = rect.bottom() - meter_position(1.0) * rect.height();
    painter.line_segment(
        [Pos2::new(bars.left(), zero), Pos2::new(bars.right(), zero)],
        Stroke::new(1.0, theme::TEXT_DIM.gamma_multiply(0.5)),
    );
    // Gain reduction hangs from the top: 0..24 dB.
    let gr = Rect::from_min_size(
        Pos2::new(rect.right() - gr_width, rect.top()),
        Vec2::new(gr_width, rect.height()),
    );
    painter.rect_filled(gr, 0.0, theme::BG_1);
    if state.reduction_db > 0.05 {
        let depth = (state.reduction_db / 24.0).clamp(0.0, 1.0) * gr.height();
        painter.rect_filled(
            Rect::from_min_size(gr.min, Vec2::new(gr_width, depth)),
            0.0,
            theme::ACCENT,
        );
    }
    response.on_hover_text(format!(
        "{}  {} dB\nGR  {:.1} dB",
        t("audio.peak"),
        format_db(state.peak_db()),
        state.reduction_db
    ))
}

/// A vertical fader with a dB scale. Drag to move (Shift: fine), double-click for 0 dB.
pub fn fader(ui: &mut egui::Ui, db: &mut f32, size: Vec2) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), Vec2::new(4.0, rect.height() - 12.0));
    let painter = ui.painter_at(rect);
    // Scale marks.
    for mark in [12.0, 6.0, 0.0, -6.0, -12.0, -24.0, -40.0, -60.0] {
        let y = track.bottom() - fader_position(mark) * track.height();
        let strong = mark == 0.0;
        painter.line_segment(
            [
                Pos2::new(rect.left() + 2.0, y),
                Pos2::new(track.left() - 3.0, y),
            ],
            Stroke::new(1.0, if strong { theme::TEXT_DIM } else { theme::BG_4 }),
        );
        painter.text(
            Pos2::new(track.right() + 4.0, y),
            Align2::LEFT_CENTER,
            format!("{}", mark as i32),
            FontId::proportional(8.5),
            if strong { theme::TEXT_DIM } else { theme::BG_4 },
        );
    }
    painter.rect_filled(track, 2.0, theme::BG_0);
    if response.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let delta = response.drag_delta().y / track.height() * if fine { 0.2 } else { 1.0 };
        let position = (fader_position(*db) - delta).clamp(0.0, 1.0);
        *db = (fader_db(position) * 10.0).round() / 10.0;
        response.mark_changed();
    }
    if response.double_clicked() {
        *db = 0.0;
        response.mark_changed();
    }
    let y = track.bottom() - fader_position(*db) * track.height();
    let cap = Rect::from_center_size(
        Pos2::new(track.center().x, y),
        Vec2::new(rect.width().min(26.0), 10.0),
    );
    let hot = response.hovered() || response.dragged();
    painter.rect_filled(cap, 2.0, if hot { theme::BG_4 } else { theme::BG_3 });
    painter.line_segment(
        [
            Pos2::new(cap.left() + 2.0, y),
            Pos2::new(cap.right() - 2.0, y),
        ],
        Stroke::new(1.5, theme::TEXT),
    );
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    response.on_hover_text(format!("{} dB", format_db(*db)))
}

// =============================================================================================
// Equalizer
// =============================================================================================

/// The spectrum of a bus, in log-spaced bands, smoothed over time.
#[derive(Debug, Clone, Default)]
pub struct Spectrum {
    pub bands: Vec<f32>,
    samples: Vec<f32>,
}

/// Bands the spectrum shows across 20 Hz–20 kHz.
const SPECTRUM_BANDS: usize = 160;

impl Spectrum {
    /// Analyzes the bus's latest samples (rises at once, falls ~40 dB/s).
    pub fn update(&mut self, levels: &ab_engine::audio::Levels, delta: f32) {
        levels.recent_signal(&mut self.samples);
        let n = self.samples.len();
        if n < 64 || !n.is_power_of_two() {
            return;
        }
        let rate = levels.sample_rate() as f32;
        let mut re: Vec<f32> = self
            .samples
            .iter()
            .enumerate()
            .map(|(i, s)| {
                // Hann window.
                let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (n - 1) as f32).cos();
                s * w
            })
            .collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        let magnitude =
            |bin: usize| (re[bin] * re[bin] + im[bin] * im[bin]).sqrt() * 4.0 / n as f32;
        self.bands
            .resize(SPECTRUM_BANDS, ab_engine::audio::dsp::SILENCE_DB);
        let fall = 40.0 * delta.clamp(0.0, 0.2);
        let (lo, hi) = (20f32.log10(), 20_000f32.log10());
        for (b, band) in self.bands.iter_mut().enumerate() {
            let f0 = 10f32.powf(lo + (hi - lo) * b as f32 / SPECTRUM_BANDS as f32);
            let f1 = 10f32.powf(lo + (hi - lo) * (b + 1) as f32 / SPECTRUM_BANDS as f32);
            let bin0 = ((f0 / rate * n as f32) as usize).clamp(1, n / 2 - 1);
            let bin1 = ((f1 / rate * n as f32) as usize).clamp(bin0, n / 2 - 1);
            let level = (bin0..=bin1).map(magnitude).fold(0.0, f32::max);
            let db = gain_to_db(level);
            *band = if db > *band {
                db
            } else {
                (*band - fall).max(db)
            };
        }
    }
}

/// In-place radix-2 FFT (lengths that are powers of two).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -std::f32::consts::TAU / len as f32;
        let (w_re, w_im) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut c_re, mut c_im) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let t_re = re[b] * c_re - im[b] * c_im;
                let t_im = re[b] * c_im + im[b] * c_re;
                re[b] = re[a] - t_re;
                im[b] = im[a] - t_im;
                re[a] += t_re;
                im[a] += t_im;
                let next = c_re * w_re - c_im * w_im;
                c_im = c_re * w_im + c_im * w_re;
                c_re = next;
            }
        }
        len <<= 1;
    }
}

const BAND_COLORS: [Color32; 8] = [
    Color32::from_rgb(235, 99, 99),
    Color32::from_rgb(240, 160, 70),
    Color32::from_rgb(230, 210, 80),
    Color32::from_rgb(110, 205, 110),
    Color32::from_rgb(80, 200, 210),
    Color32::from_rgb(90, 140, 245),
    Color32::from_rgb(170, 110, 240),
    Color32::from_rgb(235, 110, 200),
];

/// The color of band `i` (the curve's handle and its row).
pub fn band_color(i: usize) -> Color32 {
    BAND_COLORS[i % BAND_COLORS.len()]
}

/// Most gain the EQ graph shows, ± dB.
const EQ_RANGE_DB: f32 = 24.0;

/// The equalizer's response over a grid, the spectrum behind it and a handle per band:
/// drag a handle (frequency and gain), scroll on it (Q), double-click it (on/off),
/// right-click it (shape, delete), double-click empty space to add a band.
pub fn eq_graph(
    ui: &mut egui::Ui,
    eq: &mut EqSettings,
    selected: &mut Option<usize>,
    spectrum: Option<&Spectrum>,
    sample_rate: f32,
    height: f32,
) -> bool {
    let size = Vec2::new(ui.available_width(), height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, theme::BG_0);
    // Zoomed to the bands: ±12 dB, more when a band reaches further.
    let reach = eq
        .bands
        .iter()
        .filter(|b| b.enabled && b.shape.has_gain())
        .map(|b| b.gain_db.abs())
        .fold(eq.output_db.abs(), f32::max);
    let range = ((reach + 3.0) / 6.0).ceil().clamp(2.0, EQ_RANGE_DB / 6.0) * 6.0;
    let y_of = |db: f32| rect.center().y - db / range * rect.height() * 0.5;
    let db_of = |y: f32| (rect.center().y - y) / (rect.height() * 0.5) * range;

    // Grid.
    for freq in [
        50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 10_000.0,
    ] {
        let x = freq_x(rect, freq);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.0, theme::BG_2),
        );
        painter.text(
            Pos2::new(x + 2.0, rect.bottom() - 2.0),
            Align2::LEFT_BOTTOM,
            format_freq(freq),
            FontId::proportional(9.0),
            theme::BG_4,
        );
    }
    let step = if range > 12.0 { 6.0 } else { 3.0 };
    for i in -8..=8 {
        let db = i as f32 * step;
        if db.abs() >= range {
            continue;
        }
        let y = y_of(db);
        let color = if db == 0.0 { theme::BG_4 } else { theme::BG_2 };
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(1.0, color),
        );
        painter.text(
            Pos2::new(rect.left() + 2.0, y - 1.0),
            Align2::LEFT_BOTTOM,
            format!("{db:+}"),
            FontId::proportional(9.0),
            theme::BG_4,
        );
    }

    // Spectrum (its level mapped from -90..0 dBFS onto the graph's height).
    if let Some(spectrum) = spectrum.filter(|s| !s.bands.is_empty()) {
        let n = spectrum.bands.len();
        let mut points = Vec::with_capacity(n + 2);
        points.push(rect.left_bottom());
        for (i, &db) in spectrum.bands.iter().enumerate() {
            let x = rect.left() + (i as f32 + 0.5) / n as f32 * rect.width();
            let t = ((db + 90.0) / 90.0).clamp(0.0, 1.0);
            points.push(Pos2::new(x, rect.bottom() - t * rect.height()));
        }
        points.push(rect.right_bottom());
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            painter.add(Shape::convex_polygon(
                vec![
                    Pos2::new(a.x, rect.bottom()),
                    a,
                    b,
                    Pos2::new(b.x, rect.bottom()),
                ],
                Color32::from_rgba_unmultiplied(120, 160, 255, 28),
                Stroke::NONE,
            ));
        }
        painter.add(Shape::line(
            points[1..points.len() - 1].to_vec(),
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(140, 180, 255, 90)),
        ));
    }

    // Each band's own curve, faint; the sum, bright and filled.
    let columns = (rect.width() / 2.0).max(2.0) as usize;
    let freq_at = |c: usize| x_freq(rect, rect.left() + c as f32 / columns as f32 * rect.width());
    for (i, band) in eq.bands.iter().enumerate() {
        if !band.enabled {
            continue;
        }
        let points: Vec<Pos2> = (0..=columns)
            .map(|c| {
                let f = freq_at(c);
                Pos2::new(
                    freq_x(rect, f),
                    y_of(band_response_db(band, f, sample_rate).clamp(-60.0, 60.0)),
                )
            })
            .collect();
        let alpha = if *selected == Some(i) { 200 } else { 70 };
        let c = band_color(i);
        painter.add(Shape::line(
            points,
            Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha),
            ),
        ));
    }
    let total: Vec<Pos2> = (0..=columns)
        .map(|c| {
            let f = freq_at(c);
            let db: f32 = eq
                .bands
                .iter()
                .map(|b| band_response_db(b, f, sample_rate))
                .sum::<f32>()
                + eq.output_db;
            Pos2::new(freq_x(rect, f), y_of(db.clamp(-60.0, 60.0)))
        })
        .collect();
    let zero = y_of(0.0);
    for pair in total.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (top, bottom) = (a.y.min(b.y).min(zero), a.y.max(b.y).max(zero));
        if (bottom - top) > 0.5 {
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(a.x, a.y.min(zero)), Pos2::new(b.x, a.y.max(zero))),
                0.0,
                Color32::from_rgba_unmultiplied(64, 138, 255, 36),
            );
        }
    }
    painter.add(Shape::line(total, Stroke::new(2.0, theme::ACCENT)));

    // Handles.
    let handle = |band: &EqBand| {
        let db = if band.shape.has_gain() {
            band.gain_db
        } else {
            0.0
        };
        Pos2::new(freq_x(rect, band.freq), y_of(db))
    };
    let pointer = response.hover_pos();
    let near = pointer.and_then(|p| {
        eq.bands
            .iter()
            .enumerate()
            .map(|(i, b)| (i, handle(b).distance(p)))
            .filter(|(_, d)| *d < 10.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    });
    let drag_id = response.id.with("eq-drag");
    let mut changed = false;
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(drag_id, near));
        if near.is_some() {
            *selected = near;
        }
    }
    let dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id)).flatten();
    if let (true, Some(i), Some(p)) = (
        response.dragged(),
        dragging,
        response.interact_pointer_pos(),
    ) {
        if let Some(band) = eq.bands.get_mut(i) {
            let (lo, hi) = EqBand::FREQ_RANGE;
            band.freq = (x_freq(rect, p.x).clamp(lo, hi) * 10.0).round() / 10.0;
            if band.shape.has_gain() {
                let (lo, hi) = EqBand::GAIN_RANGE;
                band.gain_db = (db_of(p.y).clamp(lo, hi) * 10.0).round() / 10.0;
            }
            changed = true;
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<Option<usize>>(drag_id));
    }
    if let Some(i) = near {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            if let Some(band) = eq.bands.get_mut(i) {
                let (lo, hi) = EqBand::Q_RANGE;
                band.q = (band.q * (scroll * 0.004).exp()).clamp(lo, hi);
                changed = true;
            }
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if response.clicked() {
        *selected = near;
    }
    if response.double_clicked() {
        match near {
            Some(i) => {
                eq.bands[i].enabled = !eq.bands[i].enabled;
                changed = true;
            }
            None if eq.bands.len() < EqSettings::MAX_BANDS => {
                if let Some(p) = pointer {
                    eq.bands.push(EqBand {
                        freq: x_freq(rect, p.x).round(),
                        gain_db: db_of(p.y).clamp(-EQ_RANGE_DB, EQ_RANGE_DB).round(),
                        ..EqBand::default()
                    });
                    *selected = Some(eq.bands.len() - 1);
                    changed = true;
                }
            }
            None => {}
        }
    }
    let menu_band = near.or(*selected);
    response.context_menu(|ui| {
        let Some(i) = menu_band.filter(|&i| i < eq.bands.len()) else {
            ui.label(t("audio.eq_hint"));
            return;
        };
        for shape in FilterShape::ALL {
            if ui
                .radio(eq.bands[i].shape == shape, shape_name(shape))
                .clicked()
            {
                eq.bands[i].shape = shape;
                changed = true;
                ui.close();
            }
        }
        ui.separator();
        if ui.button(t("audio.remove_band")).clicked() {
            eq.bands.remove(i);
            *selected = None;
            changed = true;
            ui.close();
        }
    });
    for (i, band) in eq.bands.iter().enumerate() {
        let p = handle(band);
        let color = band_color(i);
        let fill = if band.enabled { color } else { theme::BG_3 };
        let radius = if *selected == Some(i) || near == Some(i) {
            7.0
        } else {
            5.5
        };
        painter.circle(
            p,
            radius,
            fill,
            Stroke::new(
                1.5,
                if *selected == Some(i) {
                    Color32::WHITE
                } else {
                    theme::BG_0
                },
            ),
        );
        painter.text(
            p,
            Align2::CENTER_CENTER,
            format!("{}", i + 1),
            FontId::proportional(8.0),
            theme::BG_0,
        );
    }
    if let Some(i) = near.or(dragging) {
        if let Some(band) = eq.bands.get(i) {
            let text = if band.shape.has_gain() {
                format!(
                    "{} Hz  {:+.1} dB  Q {:.2}",
                    format_freq(band.freq),
                    band.gain_db,
                    band.q
                )
            } else {
                format!("{} Hz  Q {:.2}", format_freq(band.freq), band.q)
            };
            painter.text(
                rect.right_top() + Vec2::new(-6.0, 4.0),
                Align2::RIGHT_TOP,
                text,
                FontId::monospace(11.0),
                theme::TEXT,
            );
        }
    }
    changed
}

/// The name of a filter shape in the editor's language.
pub fn shape_name(shape: FilterShape) -> String {
    t(match shape {
        FilterShape::Peak => "audio.shape_peak",
        FilterShape::LowShelf => "audio.shape_low_shelf",
        FilterShape::HighShelf => "audio.shape_high_shelf",
        FilterShape::LowCut => "audio.shape_low_cut",
        FilterShape::HighCut => "audio.shape_high_cut",
        FilterShape::BandPass => "audio.shape_band_pass",
        FilterShape::Notch => "audio.shape_notch",
    })
}

/// One row per band: on, shape, frequency, gain, Q, slope.
pub fn eq_bands(ui: &mut egui::Ui, eq: &mut EqSettings, selected: &mut Option<usize>) -> bool {
    let mut changed = false;
    let mut remove = None;
    egui::Grid::new("eq_bands")
        .num_columns(7)
        .spacing([6.0, 3.0])
        .show(ui, |ui| {
            for (i, band) in eq.bands.iter_mut().enumerate() {
                let color = band_color(i);
                if ui
                    .selectable_label(
                        *selected == Some(i),
                        egui::RichText::new(format!("● {}", i + 1)).color(color),
                    )
                    .clicked()
                {
                    *selected = Some(i);
                }
                changed |= ui.checkbox(&mut band.enabled, "").changed();
                egui::ComboBox::from_id_salt(("eq_shape", i))
                    .selected_text(shape_name(band.shape))
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for shape in FilterShape::ALL {
                            changed |= ui
                                .selectable_value(&mut band.shape, shape, shape_name(shape))
                                .changed();
                        }
                    });
                let (lo, hi) = EqBand::FREQ_RANGE;
                changed |= ui
                    .add({
                        let speed = f64::from(band.freq) * 0.005;
                        egui::DragValue::new(&mut band.freq)
                            .range(lo..=hi)
                            .speed(speed)
                            .suffix(" Hz")
                            .max_decimals(1)
                    })
                    .changed();
                ui.add_enabled_ui(band.shape.has_gain(), |ui| {
                    let (lo, hi) = EqBand::GAIN_RANGE;
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut band.gain_db)
                                .range(lo..=hi)
                                .speed(0.1)
                                .suffix(" dB")
                                .max_decimals(1),
                        )
                        .changed();
                });
                let (lo, hi) = EqBand::Q_RANGE;
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut band.q)
                            .range(lo..=hi)
                            .speed(0.01)
                            .prefix("Q ")
                            .max_decimals(2),
                    )
                    .changed();
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(band.shape.has_slope(), |ui| {
                        egui::ComboBox::from_id_salt(("eq_slope", i))
                            .selected_text(format!("{} dB/oct", band.slope))
                            .width(84.0)
                            .show_ui(ui, |ui| {
                                for slope in [12u8, 24, 36, 48] {
                                    changed |= ui
                                        .selectable_value(
                                            &mut band.slope,
                                            slope,
                                            format!("{slope} dB/oct"),
                                        )
                                        .changed();
                                }
                            });
                    });
                    if ui
                        .small_button(egui_phosphor::regular::X)
                        .on_hover_text(t("audio.remove_band"))
                        .clicked()
                    {
                        remove = Some(i);
                    }
                });
                ui.end_row();
            }
        });
    if let Some(i) = remove {
        eq.bands.remove(i);
        *selected = None;
        changed = true;
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                eq.bands.len() < EqSettings::MAX_BANDS,
                egui::Button::new(format!(
                    "{}  {}",
                    egui_phosphor::regular::PLUS,
                    t("audio.add_band")
                )),
            )
            .clicked()
        {
            eq.bands.push(EqBand::default());
            *selected = Some(eq.bands.len() - 1);
            changed = true;
        }
        ui.label(egui::RichText::new(t("audio.output")).color(theme::TEXT_DIM));
        changed |= ui
            .add(
                egui::DragValue::new(&mut eq.output_db)
                    .range(-24.0..=24.0)
                    .speed(0.1)
                    .suffix(" dB"),
            )
            .changed();
    });
    changed
}

// =============================================================================================
// Dynamics
// =============================================================================================

/// The compressor's input → output curve, with where the signal is now.
pub fn compressor_graph(ui: &mut egui::Ui, c: &CompressorSettings, reduction_db: f32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, theme::BG_0);
    let range = 60.0;
    let to_screen = |input: f32, output: f32| {
        Pos2::new(
            rect.left() + (input + range) / range * rect.width(),
            rect.bottom() - (output + range) / range * rect.height(),
        )
    };
    for db in [-48.0, -36.0, -24.0, -12.0] {
        let a = to_screen(db, -range);
        let b = to_screen(db, 0.0);
        painter.line_segment([a, b], Stroke::new(1.0, theme::BG_2));
        let a = to_screen(-range, db);
        let b = to_screen(0.0, db);
        painter.line_segment([a, b], Stroke::new(1.0, theme::BG_2));
    }
    painter.line_segment(
        [to_screen(-range, -range), to_screen(0.0, 0.0)],
        Stroke::new(1.0, theme::BG_4),
    );
    let t_x = to_screen(c.threshold_db, -range).x;
    painter.line_segment(
        [Pos2::new(t_x, rect.top()), Pos2::new(t_x, rect.bottom())],
        Stroke::new(1.0, theme::WARN.gamma_multiply(0.6)),
    );
    let points: Vec<Pos2> = (0..=120)
        .map(|i| {
            let input = -range + i as f32 * range / 120.0;
            to_screen(
                input,
                compressor_curve(input, c.threshold_db, c.ratio, c.knee_db) + c.makeup_db,
            )
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(2.0, theme::ACCENT)));
    if reduction_db > 0.05 {
        painter.text(
            rect.left_top() + Vec2::new(4.0, 4.0),
            Align2::LEFT_TOP,
            format!("GR {reduction_db:.1} dB"),
            FontId::monospace(10.0),
            theme::ACCENT,
        );
    }
}

// =============================================================================================
// Effect parameters
// =============================================================================================

fn row(ui: &mut egui::Ui, name: &str, add: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
    crate::panels::label(ui, name);
    let changed = add(ui);
    ui.end_row();
    changed
}

fn number(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    speed: f64,
    suffix: &str,
) -> bool {
    ui.add(
        egui::DragValue::new(value)
            .custom_parser(crate::panels::parse_finite)
            .range(range)
            .speed(speed)
            .suffix(suffix)
            .max_decimals(2),
    )
    .changed()
}

fn amount(ui: &mut egui::Ui, value: &mut f32) -> bool {
    ui.add(
        egui::Slider::new(value, 0.0..=1.0)
            .show_value(true)
            .fixed_decimals(2),
    )
    .changed()
}

/// Every setting of an effect (the EQ's bands are edited with [`eq_graph`]).
/// `buses` are the names a compressor's sidechain can listen to.
pub fn effect_params(
    ui: &mut egui::Ui,
    effect: &mut EffectDesc,
    buses: &[String],
    id: &str,
) -> bool {
    let mut changed = false;
    crate::panels::grid(ui, id, |ui| match &mut effect.kind {
        EffectKind::Eq(_) => {}
        EffectKind::Filter(band) => {
            changed |= row(ui, "Shape", |ui| {
                let mut c = false;
                egui::ComboBox::from_id_salt((id, "shape"))
                    .selected_text(shape_name(band.shape))
                    .show_ui(ui, |ui| {
                        for shape in FilterShape::ALL {
                            c |= ui
                                .selectable_value(&mut band.shape, shape, shape_name(shape))
                                .changed();
                        }
                    });
                c
            });
            let speed = f64::from(band.freq) * 0.005;
            changed |= row(ui, "Frequency", |ui| {
                number(ui, &mut band.freq, 20.0..=20_000.0, speed, " Hz")
            });
            changed |= row(ui, "Q", |ui| number(ui, &mut band.q, 0.1..=18.0, 0.01, ""));
            if band.shape.has_gain() {
                changed |= row(ui, "Gain", |ui| {
                    number(ui, &mut band.gain_db, -24.0..=24.0, 0.1, " dB")
                });
            }
            if band.shape.has_slope() {
                changed |= row(ui, "Slope", |ui| {
                    let mut c = false;
                    egui::ComboBox::from_id_salt((id, "slope"))
                        .selected_text(format!("{} dB/oct", band.slope))
                        .show_ui(ui, |ui| {
                            for slope in [12u8, 24, 36, 48] {
                                c |= ui
                                    .selectable_value(
                                        &mut band.slope,
                                        slope,
                                        format!("{slope} dB/oct"),
                                    )
                                    .changed();
                            }
                        });
                    c
                });
            }
        }
        EffectKind::Compressor(c) => {
            let CompressorSettings {
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                knee_db,
                makeup_db,
                mix,
                sidechain,
            } = c;
            changed |= row(ui, "Threshold", |ui| {
                number(ui, threshold_db, -80.0..=0.0, 0.1, " dB")
            });
            changed |= row(ui, "Ratio", |ui| {
                number(ui, ratio, 1.0..=100.0, 0.05, " : 1")
            });
            changed |= row(ui, "Attack", |ui| {
                number(ui, attack_ms, 0.0..=500.0, 0.1, " ms")
            });
            changed |= row(ui, "Release", |ui| {
                number(ui, release_ms, 1.0..=5_000.0, 1.0, " ms")
            });
            changed |= row(ui, "Knee", |ui| number(ui, knee_db, 0.0..=24.0, 0.1, " dB"));
            changed |= row(ui, "Makeup", |ui| {
                number(ui, makeup_db, -24.0..=36.0, 0.1, " dB")
            });
            changed |= row(ui, "Mix", |ui| amount(ui, mix));
            changed |= row(ui, "Sidechain", |ui| {
                let mut c = false;
                let none = t("audio.sidechain_none");
                egui::ComboBox::from_id_salt((id, "sidechain"))
                    .selected_text(sidechain.clone().unwrap_or_else(|| none.clone()))
                    .show_ui(ui, |ui| {
                        c |= ui.selectable_value(sidechain, None, none).changed();
                        for bus in buses {
                            c |= ui
                                .selectable_value(sidechain, Some(bus.clone()), bus)
                                .changed();
                        }
                    });
                c
            });
        }
        EffectKind::Limiter(l) => {
            let LimiterSettings {
                input_db,
                ceiling_db,
                release_ms,
                lookahead_ms,
            } = l;
            changed |= row(ui, "Input gain", |ui| {
                number(ui, input_db, -24.0..=36.0, 0.1, " dB")
            });
            changed |= row(ui, "Ceiling", |ui| {
                number(ui, ceiling_db, -30.0..=0.0, 0.05, " dB")
            });
            changed |= row(ui, "Release", |ui| {
                number(ui, release_ms, 1.0..=2_000.0, 1.0, " ms")
            });
            changed |= row(ui, "Look-ahead", |ui| {
                number(
                    ui,
                    lookahead_ms,
                    0.5..=LimiterSettings::MAX_LOOKAHEAD_MS,
                    0.05,
                    " ms",
                )
            });
        }
        EffectKind::Reverb(r) => {
            let ReverbSettings {
                room_size,
                damping,
                width,
                pre_delay_ms,
                low_cut_hz,
                high_cut_hz,
                mix,
            } = r;
            changed |= row(ui, "Room size", |ui| amount(ui, room_size));
            changed |= row(ui, "Damping", |ui| amount(ui, damping));
            changed |= row(ui, "Width", |ui| amount(ui, width));
            changed |= row(ui, "Pre-delay", |ui| {
                number(
                    ui,
                    pre_delay_ms,
                    0.0..=ReverbSettings::MAX_PRE_DELAY_MS,
                    0.5,
                    " ms",
                )
            });
            changed |= row(ui, "Low cut", |ui| {
                number(ui, low_cut_hz, 20.0..=2_000.0, 1.0, " Hz")
            });
            changed |= row(ui, "High cut", |ui| {
                number(ui, high_cut_hz, 1_000.0..=20_000.0, 10.0, " Hz")
            });
            changed |= row(ui, "Mix", |ui| amount(ui, mix));
        }
        EffectKind::Delay(d) => {
            let DelaySettings {
                time_ms,
                feedback,
                mix,
                ping_pong,
                high_cut_hz,
            } = d;
            changed |= row(ui, "Time", |ui| {
                number(ui, time_ms, 1.0..=DelaySettings::MAX_TIME_MS, 1.0, " ms")
            });
            changed |= row(ui, "Feedback", |ui| {
                ui.add(egui::Slider::new(feedback, 0.0..=0.95).fixed_decimals(2))
                    .changed()
            });
            changed |= row(ui, "Mix", |ui| amount(ui, mix));
            changed |= row(ui, "Ping-pong", |ui| ui.checkbox(ping_pong, "").changed());
            changed |= row(ui, "High cut", |ui| {
                number(ui, high_cut_hz, 500.0..=20_000.0, 10.0, " Hz")
            });
        }
        EffectKind::Saturation(s) => {
            let SaturationSettings {
                drive_db,
                mix,
                output_db,
            } = s;
            changed |= row(ui, "Drive", |ui| {
                number(ui, drive_db, 0.0..=48.0, 0.1, " dB")
            });
            changed |= row(ui, "Mix", |ui| amount(ui, mix));
            changed |= row(ui, "Output", |ui| {
                number(ui, output_db, -48.0..=12.0, 0.1, " dB")
            });
        }
        EffectKind::Utility(u) => {
            let UtilitySettings {
                gain_db,
                pan,
                width,
            } = u;
            changed |= row(ui, "Gain", |ui| {
                number(ui, gain_db, -96.0..=24.0, 0.1, " dB")
            });
            changed |= row(ui, "Pan", |ui| {
                ui.add(egui::Slider::new(pan, -1.0..=1.0).fixed_decimals(2))
                    .changed()
            });
            changed |= row(ui, "Width", |ui| {
                ui.add(egui::Slider::new(width, 0.0..=2.0).fixed_decimals(2))
                    .changed()
            });
        }
    });
    changed
}

/// The name of an effect kind in the editor's language.
pub fn effect_name(kind: &str) -> String {
    t(&format!("audio.effect_{kind}"))
}

// =============================================================================================
// Distance rolloff
// =============================================================================================

/// Volume over distance; with a custom rolloff its points can be dragged, added
/// (double-click) and removed (right-click).
pub fn attenuation_graph(ui: &mut egui::Ui, a: &mut Attenuation, height: f32) -> bool {
    let size = Vec2::new(ui.available_width(), height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, theme::BG_0);
    let max = a.max_distance.max(a.min_distance + 0.01).max(0.01) * 1.1;
    let x_of = |d: f32| rect.left() + d / max * rect.width();
    let y_of = |g: f32| rect.bottom() - g * rect.height();
    for g in [0.25, 0.5, 0.75] {
        painter.line_segment(
            [
                Pos2::new(rect.left(), y_of(g)),
                Pos2::new(rect.right(), y_of(g)),
            ],
            Stroke::new(1.0, theme::BG_2),
        );
    }
    for (d, label) in [(a.min_distance, "min"), (a.max_distance, "max")] {
        let x = x_of(d);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.0, theme::BG_4),
        );
        painter.text(
            Pos2::new(x + 3.0, rect.top() + 2.0),
            Align2::LEFT_TOP,
            format!("{label} {d:.1} m"),
            FontId::proportional(9.0),
            theme::TEXT_DIM,
        );
    }
    let points: Vec<Pos2> = (0..=160)
        .map(|i| {
            let d = i as f32 / 160.0 * max;
            Pos2::new(x_of(d), y_of(a.gain(d)))
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(2.0, theme::ACCENT)));

    let mut changed = false;
    if a.rolloff == Rolloff::Custom {
        let span = (a.max_distance - a.min_distance).max(0.01);
        let to_screen = |p: [f32; 2]| Pos2::new(x_of(a.min_distance + p[0] * span), y_of(p[1]));
        let from_screen = |p: Pos2| {
            [
                (((p.x - rect.left()) / rect.width() * max - a.min_distance) / span)
                    .clamp(0.0, 1.0),
                ((rect.bottom() - p.y) / rect.height()).clamp(0.0, 1.0),
            ]
        };
        let near = response.hover_pos().and_then(|p| {
            a.curve
                .iter()
                .enumerate()
                .map(|(i, c)| (i, to_screen(*c).distance(p)))
                .filter(|(_, d)| *d < 9.0)
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .map(|(i, _)| i)
        });
        let id = response.id.with("curve-drag");
        if response.drag_started() {
            ui.data_mut(|d| d.insert_temp(id, near));
        }
        let dragging: Option<usize> = ui.data(|d| d.get_temp(id)).flatten();
        if let (true, Some(i), Some(p)) = (
            response.dragged(),
            dragging,
            response.interact_pointer_pos(),
        ) {
            if let Some(point) = a.curve.get_mut(i) {
                *point = from_screen(p);
                changed = true;
            }
        }
        if response.double_clicked() && near.is_none() {
            if let Some(p) = response.interact_pointer_pos() {
                a.curve.push(from_screen(p));
                changed = true;
            }
        }
        if response.secondary_clicked() {
            if let Some(i) = near {
                a.curve.remove(i);
                changed = true;
            }
        }
        for (i, point) in a.curve.iter().enumerate() {
            let hot = near == Some(i);
            painter.circle(
                to_screen(*point),
                if hot { 6.0 } else { 4.5 },
                theme::SELECTION,
                Stroke::new(1.0, theme::BG_0),
            );
        }
    }
    response.on_hover_text(t("audio.rolloff_hint"));
    changed
}

// =============================================================================================
// Waveform
// =============================================================================================

/// Which part of a clip a handle moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipHandle {
    Start,
    End,
    FadeIn,
    FadeOut,
    LoopStart,
    LoopEnd,
}

/// A clip's waveform with its cut: drag the edges to trim, the top corners to fade, the
/// loop markers to set the loop. Click to move the playhead (returned when clicked).
pub fn waveform_editor(
    ui: &mut egui::Ui,
    peaks: &[[f32; 2]],
    duration: f32,
    edit: &mut ClipEdit,
    playhead: Option<f32>,
    height: f32,
) -> (bool, Option<f32>) {
    let size = Vec2::new(ui.available_width(), height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, theme::BG_0);
    let duration = duration.max(1e-3);
    let x_of = |s: f32| rect.left() + (s / duration).clamp(0.0, 1.0) * rect.width();
    let s_of = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) * duration;
    let end = if edit.end > edit.start && edit.end < duration {
        edit.end
    } else {
        duration
    };
    let start = edit.start.clamp(0.0, end);
    let gain = db_to_gain(edit.volume_db);

    // The waveform, dimmed outside the cut; the cut scaled by gain and fades.
    let mid = rect.center().y;
    let half = rect.height() * 0.45;
    let n = peaks.len().max(1);
    for (i, &[lo, hi]) in peaks.iter().enumerate() {
        let x = rect.left() + (i as f32 + 0.5) / n as f32 * rect.width();
        let s = s_of(x);
        let inside = s >= start && s <= end;
        let mut g = if inside { gain } else { 1.0 };
        if inside {
            if edit.fade_in > 0.0 && s < start + edit.fade_in {
                g *= ((s - start) / edit.fade_in).clamp(0.0, 1.0);
            }
            if edit.fade_out > 0.0 && s > end - edit.fade_out {
                g *= ((end - s) / edit.fade_out).clamp(0.0, 1.0);
            }
        }
        let color = if inside { theme::ACCENT } else { theme::BG_4 };
        let top = mid - (hi * g).clamp(-1.0, 1.0) * half;
        let bottom = mid - (lo * g).clamp(-1.0, 1.0) * half;
        painter.line_segment(
            [
                Pos2::new(x, top.min(mid - 0.5)),
                Pos2::new(x, bottom.max(mid + 0.5)),
            ],
            Stroke::new(1.0, color),
        );
    }
    painter.line_segment(
        [Pos2::new(rect.left(), mid), Pos2::new(rect.right(), mid)],
        Stroke::new(1.0, theme::BG_3),
    );
    for (from, to) in [(0.0, start), (end, duration)] {
        if to > from {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(x_of(from), rect.top()),
                    Pos2::new(x_of(to), rect.bottom()),
                ),
                0.0,
                Color32::from_black_alpha(110),
            );
        }
    }
    // Fade lines.
    let fade_in_x = x_of(start + edit.fade_in);
    let fade_out_x = x_of(end - edit.fade_out);
    painter.line_segment(
        [
            Pos2::new(x_of(start), rect.bottom()),
            Pos2::new(fade_in_x, rect.top()),
        ],
        Stroke::new(1.5, theme::WARN),
    );
    painter.line_segment(
        [
            Pos2::new(fade_out_x, rect.top()),
            Pos2::new(x_of(end), rect.bottom()),
        ],
        Stroke::new(1.5, theme::WARN),
    );
    // Loop region.
    let loop_start = edit.loop_start.clamp(start, end);
    let loop_end = if edit.loop_end > loop_start && edit.loop_end < end {
        edit.loop_end
    } else {
        end
    };
    let has_loop = edit.loop_start > 0.0 || edit.loop_end > 0.0;
    if has_loop {
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x_of(loop_start), rect.bottom() - 6.0),
                Pos2::new(x_of(loop_end), rect.bottom()),
            ),
            0.0,
            theme::PLAY.gamma_multiply(0.6),
        );
    }
    // Handles.
    let handles = [
        (ClipHandle::Start, Pos2::new(x_of(start), mid)),
        (ClipHandle::End, Pos2::new(x_of(end), mid)),
        (ClipHandle::FadeIn, Pos2::new(fade_in_x, rect.top() + 5.0)),
        (ClipHandle::FadeOut, Pos2::new(fade_out_x, rect.top() + 5.0)),
        (
            ClipHandle::LoopStart,
            Pos2::new(x_of(loop_start), rect.bottom() - 4.0),
        ),
        (
            ClipHandle::LoopEnd,
            Pos2::new(x_of(loop_end), rect.bottom() - 4.0),
        ),
    ];
    let near = response.hover_pos().and_then(|p| {
        handles
            .iter()
            .map(|(h, at)| {
                // Edges grab along their whole height.
                let d = match h {
                    ClipHandle::Start | ClipHandle::End => (at.x - p.x).abs(),
                    _ => at.distance(p),
                };
                (*h, d)
            })
            .filter(|(_, d)| *d < 7.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h)
    });
    let id = response.id.with("clip-drag");
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(id, near));
    }
    let dragging: Option<ClipHandle> = ui.data(|d| d.get_temp(id)).flatten();
    let mut changed = false;
    if let (true, Some(h), Some(p)) = (
        response.dragged(),
        dragging,
        response.interact_pointer_pos(),
    ) {
        let s = (s_of(p.x) * 1000.0).round() / 1000.0;
        match h {
            ClipHandle::Start => edit.start = s.min(end - 0.001).max(0.0),
            ClipHandle::End => {
                edit.end = if s >= duration - 0.0005 {
                    0.0
                } else {
                    s.max(start + 0.001)
                }
            }
            ClipHandle::FadeIn => {
                edit.fade_in = (s - start).clamp(0.0, end - start - edit.fade_out)
            }
            ClipHandle::FadeOut => edit.fade_out = (end - s).clamp(0.0, end - start - edit.fade_in),
            ClipHandle::LoopStart => edit.loop_start = s.clamp(start, loop_end - 0.001),
            ClipHandle::LoopEnd => {
                edit.loop_end = if s >= end - 0.0005 {
                    0.0
                } else {
                    s.max(loop_start + 0.001)
                }
            }
        }
        changed = true;
    }
    if near.is_some() || dragging.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    for (h, at) in handles {
        let hot = near == Some(h) || dragging == Some(h);
        match h {
            ClipHandle::Start | ClipHandle::End => {
                painter.line_segment(
                    [Pos2::new(at.x, rect.top()), Pos2::new(at.x, rect.bottom())],
                    Stroke::new(if hot { 2.5 } else { 1.5 }, theme::TEXT),
                );
            }
            ClipHandle::FadeIn | ClipHandle::FadeOut => {
                painter.circle_filled(at, if hot { 5.5 } else { 4.0 }, theme::WARN);
            }
            ClipHandle::LoopStart | ClipHandle::LoopEnd if has_loop || hot => {
                painter.circle_filled(at, if hot { 5.5 } else { 4.0 }, theme::PLAY);
            }
            _ => {}
        }
    }
    if let Some(at) = playhead {
        let x = x_of(at);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.5, Color32::WHITE),
        );
    }
    painter.text(
        rect.right_bottom() + Vec2::new(-6.0, -10.0),
        Align2::RIGHT_BOTTOM,
        format!("{:.3} – {:.3} s", start, end),
        FontId::monospace(10.0),
        theme::TEXT_DIM,
    );
    let clicked = (response.clicked() && near.is_none())
        .then(|| response.interact_pointer_pos().map(|p| s_of(p.x)))
        .flatten();
    response.on_hover_text(t("audio.waveform_hint"));
    (changed, clicked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fader_scale_round_trips_and_puts_unity_high() {
        for db in [-79.0, -60.0, -24.0, -6.0, 0.0, 6.0, 12.0] {
            assert!((fader_db(fader_position(db)) - db).abs() < 0.01, "{db}");
        }
        let unity = fader_position(0.0);
        assert!((0.7..0.85).contains(&unity), "{unity}");
        assert_eq!(fader_position(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn the_fft_finds_a_sine_in_its_bin() {
        let n = 1024;
        let mut re: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 64.0 * i as f32 / n as f32).sin())
            .collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        let mag = |k: usize| (re[k] * re[k] + im[k] * im[k]).sqrt();
        assert!((mag(64) - n as f32 / 2.0).abs() < 1.0);
        assert!(mag(63) < 1e-2 && mag(100) < 1e-2);
    }

    #[test]
    fn frequencies_map_to_the_graph_and_back() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 100.0));
        for f in [20.0, 440.0, 20_000.0] {
            assert!((x_freq(rect, freq_x(rect, f)) - f).abs() / f < 1e-3);
        }
        assert_eq!(format_db(-90.0), "-∞");
        assert_eq!(format_freq(2_500.0), "2.5k");
    }
}
