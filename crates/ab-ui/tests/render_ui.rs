//! GPU tests: lay out UI, render it offscreen and inspect the pixels. They skip on machines
//! without a usable GPU adapter (a software rasterizer counts as none). Set
//! `AB_TEST_DUMP=<dir>` to save the images.

use ab_core::{Color, Vec2};
use ab_ecs::World;
use ab_rendering::test_gpu_context as gpu;
use ab_rendering::{Environment, GpuContext, OffscreenTarget, RendererConfig, SceneRenderer};
use ab_scene::{NodeHandle, SceneTree};
use ab_ui::{
    prepare_ui, Control, HAlign, Label, LayoutPreset, Panel, ProgressBar, StyleBox, TextStyle,
    UiNodes, VAlign,
};
use std::path::PathBuf;

struct Image {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 4]>,
}

impl Image {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        self.pixels[(y * self.width + x) as usize]
    }

    /// Red channel of a pixel as 0..1 (the tests draw white or red-tinted text on black).
    fn red(&self, x: u32, y: u32) -> f32 {
        f32::from(self.at(x, y)[0]) / 255.0
    }

    /// Bounding box (x0, y0, x1, y1) of pixels whose red channel exceeds `threshold`,
    /// within `area`.
    fn ink_bounds(&self, area: [u32; 4], threshold: f32) -> Option<[u32; 4]> {
        let mut bounds: Option<[u32; 4]> = None;
        for y in area[1]..area[3] {
            for x in area[0]..area[2] {
                if self.red(x, y) > threshold {
                    let b = bounds.get_or_insert([x, y, x, y]);
                    b[0] = b[0].min(x);
                    b[1] = b[1].min(y);
                    b[2] = b[2].max(x);
                    b[3] = b[3].max(y);
                }
            }
        }
        bounds
    }
}

fn render(gpu: &GpuContext, world: &World, width: u32, height: u32, name: &str) -> Image {
    prepare_ui(world, width, height);
    let mut renderer = SceneRenderer::new(gpu, OffscreenTarget::FORMAT, &RendererConfig::default());
    let target = OffscreenTarget::new(gpu, width, height);
    renderer.render(gpu, world, target.view(), width, height);
    let pixels = target.read_pixels(gpu).unwrap();
    if let Some(dir) = std::env::var_os("AB_TEST_DUMP") {
        let bytes: Vec<u8> = pixels.iter().flatten().copied().collect();
        let path = PathBuf::from(dir).join(format!("{name}.png"));
        image::save_buffer(
            &path,
            &bytes,
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    }
    Image {
        width,
        height,
        pixels,
    }
}

fn ui_tree() -> SceneTree {
    let mut tree = SceneTree::new();
    tree.world_mut().insert_resource(Environment {
        background: Color::BLACK,
        ..Default::default()
    });
    ab_ui::register(tree.world_mut());
    tree
}

struct Hud {
    timer: NodeHandle,
    bar: NodeHandle,
}

/// Timer label top-left, progress bar (30%, red fill) along the bottom.
fn hud(tree: &mut SceneTree) -> Hud {
    let layer = tree.create_canvas_layer("Hud", 1);
    tree.add_child(tree.root(), layer).unwrap();
    let timer = tree.create_label(
        "Timer",
        Control::at(Vec2::new(40.0, 40.0), Vec2::new(400.0, 100.0)),
        Label::new("12:34").with_style(TextStyle::new(72.0)),
    );
    tree.add_child(layer, timer).unwrap();
    let mut bar = ProgressBar::new(0.0, 100.0, 30.0);
    bar.background = StyleBox::flat(Color::rgb(0.0, 0.0, 0.2));
    bar.fill = StyleBox::flat(Color::rgb(1.0, 0.0, 0.0));
    let bar = tree.create_progress_bar(
        "Health",
        Control::new().with_preset(LayoutPreset::BottomWide, Vec2::new(0.0, 40.0), 40.0),
        bar,
    );
    tree.add_child(layer, bar).unwrap();
    Hud { timer, bar }
}

/// Widest run of partially covered pixels across the left edge of the first stroke on
/// row `y`, scanning right from `x0`.
fn edge_width(image: &Image, y: u32, x0: u32, x1: u32) -> u32 {
    let mut partial = 0;
    for x in x0..x1 {
        let v = image.red(x, y);
        if v > 0.9 {
            return partial;
        }
        if v > 0.1 {
            partial += 1;
        }
    }
    partial
}

#[test]
fn hud_is_sharp_at_1080p_and_4k() {
    let Some(gpu) = gpu() else { return };
    let mut tree = ui_tree();
    let hud = hud(&mut tree);

    let hd = render(&gpu, tree.world(), 1920, 1080, "hud_1080p");
    let uhd = render(&gpu, tree.world(), 3840, 2160, "hud_4k");

    // The timer occupies the same place relative to the screen, twice as big in pixels.
    let text_hd = hd
        .ink_bounds([0, 0, 960, 300], 0.5)
        .expect("timer text drawn");
    let text_uhd = uhd
        .ink_bounds([0, 0, 1920, 600], 0.5)
        .expect("timer text drawn");
    let size = |b: [u32; 4]| Vec2::new((b[2] - b[0]) as f32, (b[3] - b[1]) as f32);
    let ratio = size(text_uhd) / size(text_hd);
    assert!(
        (ratio - Vec2::splat(2.0)).abs().max_element() < 0.05,
        "4K text is exactly twice as large: {ratio}"
    );
    assert!((text_hd[0] as f32 - 40.0).abs() < 12.0, "{text_hd:?}");

    // Sharp at both sizes: glyph edges go from background to full ink within ~2 pixels.
    for (image, bounds, name) in [(&hd, text_hd, "1080p"), (&uhd, text_uhd, "4K")] {
        let mid = (bounds[1] + bounds[3]) / 2;
        let mut widths = Vec::new();
        for dy in [-3i32, 0, 3] {
            let y = (mid as i32 + dy) as u32;
            widths.push(edge_width(image, y, bounds[0].saturating_sub(4), bounds[2]));
        }
        assert!(
            widths.iter().all(|&w| w <= 2),
            "{name}: blurry glyph edges {widths:?}"
        );
        // Solid interior: some fully inked pixels.
        let full = (bounds[0]..bounds[2])
            .filter(|&x| image.red(x, mid) > 0.95)
            .count();
        assert!(full >= 4, "{name}: strokes are solid");
    }

    // Progress bar: 30% red fill from the left margin, dark blue track after it.
    for (image, scale) in [(&hd, 1.0f32), (&uhd, 2.0)] {
        let y = image.height - (60.0 * scale) as u32;
        let left = (40.0 * scale) as u32;
        let track = image.width - (80.0 * scale) as u32;
        let fill_end = left + (track as f32 * 0.3) as u32;
        assert!(image.at(left + 4, y)[0] > 250, "fill starts at the margin");
        assert!(image.at(fill_end - 3, y)[0] > 250, "fill reaches 30%");
        let after = image.at(fill_end + 3, y);
        assert!(
            after[0] < 5 && after[2] > 60,
            "track after the fill: {after:?}"
        );
        assert!(image.at(left - 4, y)[..3] == [0, 0, 0], "margin left clear");
    }

    // Layout is in UI units, independent of the resolution.
    let rect = tree.component::<Control>(hud.bar).unwrap().rect();
    assert_eq!(rect.position, Vec2::new(40.0, 1000.0));
    assert_eq!(rect.size, Vec2::new(1840.0, 40.0));
    assert_eq!(
        tree.component::<Control>(hud.timer)
            .unwrap()
            .rect()
            .position,
        Vec2::new(40.0, 40.0)
    );
}

/// Ink rows within columns `x0..x1`, grouped into vertical runs separated by gaps.
fn vertical_runs(image: &Image, x0: u32, x1: u32, y0: u32, y1: u32) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for y in y0..y1 {
        let ink = (x0..x1).any(|x| image.red(x, y) > 0.5);
        match (ink, runs.last_mut()) {
            (true, Some(run)) if run.1 + 1 == y => run.1 = y,
            (true, _) => runs.push((y, y)),
            _ => {}
        }
    }
    runs
}

#[test]
fn thai_marks_render_stacked() {
    let Some(gpu) = gpu() else { return };
    let mut tree = ui_tree();
    // ที่ (consonant + upper vowel + tone mark): three separate layers of ink.
    let word = tree.create_label(
        "Word",
        Control::at(Vec2::new(100.0, 50.0), Vec2::new(400.0, 300.0)),
        Label::new("ที่").with_style(TextStyle::new(200.0)),
    );
    tree.add_child(tree.root(), word).unwrap();
    let image = render(&gpu, tree.world(), 960, 540, "thai_stacked");
    let bounds = image
        .ink_bounds([0, 0, 960, 540], 0.5)
        .expect("Thai text drawn");
    let runs = vertical_runs(&image, bounds[0], bounds[2] + 1, bounds[1], bounds[3] + 1);
    assert_eq!(
        runs.len(),
        3,
        "tone mark, vowel and consonant stacked without overlapping: {runs:?}"
    );
    // The consonant is the tallest layer, at the bottom.
    let heights: Vec<u32> = runs.iter().map(|r| r.1 - r.0).collect();
    assert!(
        heights[2] > heights[0] && heights[2] > heights[1],
        "{heights:?}"
    );
}

#[test]
fn mixed_script_paragraph_renders() {
    let Some(gpu) = gpu() else { return };
    let mut tree = ui_tree();
    let panel = tree.create_panel(
        "Dialog",
        Control::new().with_preset(LayoutPreset::Center, Vec2::new(900.0, 420.0), 0.0),
        Panel {
            style: StyleBox::default().with_border(2.0, Color::rgb(0.3, 0.5, 1.0)),
        },
    );
    tree.add_child(tree.root(), panel).unwrap();
    let text = tree.create_label(
        "Text",
        Control::full_rect().with_preset(LayoutPreset::FullRect, Vec2::ZERO, 32.0),
        Label::new(
            "ภาษาไทย: น้ำใส ที่นี่ กี่โมง ปู่ ญี่ปุ่น\nMixed with English text, wrapped to the panel width so it never overflows.",
        )
        .with_style(
            TextStyle::new(40.0).with_outline(2.0, Color::rgb(0.0, 0.0, 0.3)),
        )
        .with_align(HAlign::Center, VAlign::Center)
        .with_autowrap(true),
    );
    tree.add_child(panel, text).unwrap();
    // Two frames: the first one measures the wrapped label at its final width.
    prepare_ui(tree.world(), 1920, 1080);
    let image = render(&gpu, tree.world(), 1920, 1080, "mixed_paragraph");

    // All text stays inside the panel.
    let panel_rect = tree.component::<Control>(panel).unwrap().rect();
    let ink = image
        .ink_bounds([0, 0, 1920, 1080], 0.8)
        .expect("text drawn");
    assert!(
        ink[0] as f32 >= panel_rect.position.x + 32.0 - 1.0,
        "{ink:?}"
    );
    assert!(
        (ink[2] as f32) <= panel_rect.max().x - 32.0 + 1.0,
        "{ink:?}"
    );
    assert!(ink[1] as f32 >= panel_rect.position.y, "{ink:?}");
    assert!((ink[3] as f32) <= panel_rect.max().y, "{ink:?}");
}
