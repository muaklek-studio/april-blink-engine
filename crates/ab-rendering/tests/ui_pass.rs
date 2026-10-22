//! UI pass over the scene: quads land on the right pixels with the right colors.

use ab_core::Color;
use ab_ecs::World;
use ab_rendering::{
    ColorSpace, Environment, GpuContext, OffscreenTarget, RenderAssets, RenderError,
    RendererConfig, SceneRenderer, Texture, UiDrawList, UiFill, UiQuad,
};

const SIZE: u32 = 64;

fn gpu() -> Option<GpuContext> {
    match GpuContext::new_headless() {
        Ok(gpu) => Some(gpu),
        Err(RenderError::NoAdapter) => {
            eprintln!("skipping GPU test: no adapter available");
            None
        }
        Err(e) => panic!("GPU initialisation failed: {e}"),
    }
}

fn render(gpu: &GpuContext, world: &World) -> Vec<[u8; 4]> {
    let mut renderer = SceneRenderer::new(gpu, OffscreenTarget::FORMAT, &RendererConfig::default());
    let target = OffscreenTarget::new(gpu, SIZE, SIZE);
    renderer.render(gpu, world, target.view(), SIZE, SIZE);
    target.read_pixels(gpu).unwrap()
}

fn pixel(pixels: &[[u8; 4]], x: u32, y: u32) -> [u8; 4] {
    pixels[(y * SIZE + x) as usize]
}

fn close(a: [u8; 4], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 3)
}

#[test]
fn quads_draw_over_the_scene_in_order() {
    let Some(gpu) = gpu() else { return };
    let mut world = World::new();
    world.insert_resource(Environment {
        background: Color::BLACK,
        ..Default::default()
    });
    let mut assets = RenderAssets::new();
    let checker = assets
        .add_texture(
            Texture::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255], ColorSpace::Srgb).unwrap(),
        )
        .unwrap();
    world.insert_resource(assets);

    let mut panel = UiQuad::solid([8.0, 8.0, 32.0, 32.0], Color::WHITE);
    panel.stroke_width = 4.0;
    panel.stroke_color = Color::rgb(0.0, 1.0, 0.0);
    let mut clipped = UiQuad::solid([0.0, 48.0, 64.0, 16.0], Color::rgb(1.0, 1.0, 0.0));
    clipped.clip = [0.0, 0.0, 32.0, 64.0];
    let mut half = UiQuad::solid([16.0, 16.0, 8.0, 8.0], Color::rgba(0.0, 0.0, 0.0, 0.5));
    half.corner_radius = 0.0;
    let image = UiQuad {
        fill: UiFill::Image(checker),
        ..UiQuad::solid([44.0, 8.0, 16.0, 8.0], Color::WHITE)
    };
    world.insert_resource(UiDrawList {
        quads: vec![panel, clipped, half, image],
        atlas: None,
        size: [SIZE, SIZE],
    });

    let pixels = render(&gpu, &world);
    assert!(
        close(pixel(&pixels, 2, 2), [0, 0, 0]),
        "background untouched"
    );
    assert!(close(pixel(&pixels, 30, 30), [255, 255, 255]), "panel fill");
    assert!(close(pixel(&pixels, 9, 24), [0, 255, 0]), "panel border");
    // Half-transparent black over white: 50% linear = 188 in sRGB.
    let mid = pixel(&pixels, 20, 20);
    assert!(close(mid, [188, 188, 188]), "{mid:?}");
    assert!(
        close(pixel(&pixels, 10, 56), [255, 255, 0]),
        "inside the clip"
    );
    assert!(close(pixel(&pixels, 40, 56), [0, 0, 0]), "clipped away");
    assert!(
        close(pixel(&pixels, 45, 12), [255, 0, 0]),
        "image left texel"
    );
    assert!(
        close(pixel(&pixels, 57, 12), [0, 0, 255]),
        "image right texel"
    );
}

#[test]
fn rounded_corners_are_cut() {
    let Some(gpu) = gpu() else { return };
    let mut world = World::new();
    world.insert_resource(Environment {
        background: Color::BLACK,
        ..Default::default()
    });
    let mut quad = UiQuad::solid([0.0, 0.0, 64.0, 64.0], Color::WHITE);
    quad.corner_radius = 20.0;
    world.insert_resource(UiDrawList {
        quads: vec![quad],
        atlas: None,
        size: [SIZE, SIZE],
    });
    let pixels = render(&gpu, &world);
    assert!(pixel(&pixels, 1, 1)[0] < 10, "corner cut");
    assert!(pixel(&pixels, 32, 1)[0] > 245, "edge middle filled");
    assert!(pixel(&pixels, 32, 32)[0] > 245);
}
