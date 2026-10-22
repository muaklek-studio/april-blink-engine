//! Per-frame layout and draw-list generation.

use crate::control::{Control, MouseFilter, Rect};
use crate::font::{Fonts, BAKE_EM_PX, DISTANCE_RANGE};
use crate::text::{HAlign, TextBlock, TextStyle, VAlign};
use crate::widgets::{
    BoxContainer, Button, CanvasLayer, Label, Panel, ProgressBar, StretchMode, StyleBox,
    TextureRect,
};
use crate::UiState;
use ab_core::{Color, Vec2};
use ab_ecs::{Entity, SparseSet, Without, World};
use ab_rendering::{RenderAssets, UiDrawList, UiFill, UiQuad};
use ab_scene::{Children, NodeInfo, Parent};
use std::cell::{Ref, RefMut};

/// How UI units map to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScaleMode {
    /// Scale so the reference size fits the screen (letterbox-safe): `min(w / rw, h / rh)`.
    Fit,
    /// Scale with the screen height only (common for HUDs).
    Height,
    /// Scale with the screen width only.
    Width,
    /// A fixed number of pixels per UI unit.
    Fixed(f32),
}

/// ECS resource: resolution independence. Layout is done in UI units at
/// `reference_size`; at other resolutions everything (including text, which stays sharp
/// thanks to the distance-field glyphs) is scaled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiSettings {
    pub reference_size: Vec2,
    pub scale_mode: ScaleMode,
}

impl UiSettings {
    /// Pixels per UI unit for a screen of `viewport` pixels.
    pub fn scale(&self, viewport: Vec2) -> f32 {
        let ratio = viewport / self.reference_size.max(Vec2::ONE);
        let scale = match self.scale_mode {
            ScaleMode::Fit => ratio.min_element(),
            ScaleMode::Height => ratio.y,
            ScaleMode::Width => ratio.x,
            ScaleMode::Fixed(scale) => scale,
        };
        if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        }
    }
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            reference_size: Vec2::new(1920.0, 1080.0),
            scale_mode: ScaleMode::Fit,
        }
    }
}

/// A control that can be hit by the pointer or take focus, in draw order.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HitTarget {
    pub entity: Entity,
    /// Visible part of the control (clipped), in UI units.
    pub rect: Rect,
    pub blocks: bool,
    pub focusable: bool,
}

/// Lays out every control for a `width` × `height` pixel target and rebuilds the
/// [`UiDrawList`] resource. The engine calls it right before rendering a frame; call it
/// yourself when driving a renderer manually.
pub fn prepare_ui(world: &World, width: u32, height: u32) {
    let (Some(mut fonts), Some(mut list), Some(mut state)) = (
        world.resource_mut::<Fonts>(),
        world.resource_mut::<UiDrawList>(),
        world.resource_mut::<UiState>(),
    ) else {
        return;
    };
    let settings = world
        .resource::<UiSettings>()
        .map(|s| *s)
        .unwrap_or_default();
    let viewport = Vec2::new(width.max(1) as f32, height.max(1) as f32);
    let scale = settings.scale(viewport);
    state.scale = scale;
    state.viewport = viewport;
    state.targets.clear();
    list.clear();
    list.size = [width, height];

    let roots = ui_roots(world);
    let Some(controls) = world.storage_mut::<Control>() else {
        list.atlas = None;
        return;
    };
    let assets = world.resource::<RenderAssets>();
    let mut ctx = Context {
        controls,
        children: world.storage::<Children>(),
        infos: world.storage::<NodeInfo>(),
        labels: world.storage_mut::<Label>(),
        buttons: world.storage_mut::<Button>(),
        bars: world.storage_mut::<ProgressBar>(),
        panels: world.storage::<Panel>(),
        boxes: world.storage::<BoxContainer>(),
        images: world.storage::<TextureRect>(),
        assets: assets.as_deref(),
        fonts: &mut fonts,
        quads: &mut list.quads,
        targets: &mut state.targets,
        scale,
    };
    let screen = Rect::new(Vec2::ZERO, viewport / scale);
    for (_, root) in &roots {
        ctx.measure(*root);
        let rect = anchored(ctx.control(*root), &screen);
        ctx.place(*root, rect);
    }
    let no_clip = Rect::new(Vec2::splat(-1e9), Vec2::splat(2e9));
    for (_, root) in &roots {
        ctx.draw(*root, Color::WHITE, no_clip);
    }
    let has_glyphs = ctx.quads.iter().any(|q| q.fill == UiFill::Glyph);
    drop(ctx);
    list.atlas = has_glyphs.then(|| fonts.atlas_image());
}

/// Top-level controls (whose parent is not a control) inside the tree, sorted by canvas
/// layer, then tree order. Hidden nodes hide their subtree.
fn ui_roots(world: &World) -> Vec<(i32, Entity)> {
    let (Some(infos), Some(children), Some(controls)) = (
        world.storage::<NodeInfo>(),
        world.storage::<Children>(),
        world.storage::<Control>(),
    ) else {
        return Vec::new();
    };
    let layers = world.storage::<CanvasLayer>();
    let mut roots = Vec::new();
    let tree_roots: Vec<Entity> = world
        .query_filtered::<(Entity, &NodeInfo), Without<Parent>>()
        .iter()
        .filter(|(_, info)| info.in_tree)
        .map(|(e, _)| e)
        .collect();
    // (entity, layer)
    let mut stack: Vec<(Entity, i32)> = tree_roots.into_iter().rev().map(|e| (e, 0)).collect();
    while let Some((entity, layer)) = stack.pop() {
        if !infos.get(entity).is_some_and(|i| i.visible) {
            continue;
        }
        if controls.contains(entity) {
            roots.push((layer, entity));
            continue;
        }
        let layer = layers
            .as_ref()
            .and_then(|l| l.get(entity))
            .map_or(layer, |l| l.layer);
        if let Some(Children(kids)) = children.get(entity) {
            stack.extend(kids.iter().rev().map(|k| (k.entity(), layer)));
        }
    }
    roots.sort_by_key(|(layer, _)| *layer);
    roots
}

/// The rect a control gets from its anchors and offsets inside `parent`, grown to its
/// minimum size towards the right/bottom.
fn anchored(control: &Control, parent: &Rect) -> Rect {
    let [al, at, ar, ab] = control.anchors;
    let [ol, ot, or, ob] = control.offsets;
    let min = Vec2::new(
        parent.position.x + al * parent.size.x + ol,
        parent.position.y + at * parent.size.y + ot,
    );
    let max = Vec2::new(
        parent.position.x + ar * parent.size.x + or,
        parent.position.y + ab * parent.size.y + ob,
    );
    Rect::new(min, (max - min).max(control.combined_min_size()))
}

struct Context<'w, 'a> {
    controls: RefMut<'w, SparseSet<Control>>,
    children: Option<Ref<'w, SparseSet<Children>>>,
    infos: Option<Ref<'w, SparseSet<NodeInfo>>>,
    labels: Option<RefMut<'w, SparseSet<Label>>>,
    buttons: Option<RefMut<'w, SparseSet<Button>>>,
    bars: Option<RefMut<'w, SparseSet<ProgressBar>>>,
    panels: Option<Ref<'w, SparseSet<Panel>>>,
    boxes: Option<Ref<'w, SparseSet<BoxContainer>>>,
    images: Option<Ref<'w, SparseSet<TextureRect>>>,
    assets: Option<&'a RenderAssets>,
    fonts: &'a mut Fonts,
    quads: &'a mut Vec<UiQuad>,
    targets: &'a mut Vec<HitTarget>,
    scale: f32,
}

impl Context<'_, '_> {
    fn control(&self, entity: Entity) -> &Control {
        self.controls
            .get(entity)
            .expect("only entities with a Control are visited")
    }

    fn control_mut(&mut self, entity: Entity) -> &mut Control {
        self.controls
            .get_mut(entity)
            .expect("only entities with a Control are visited")
    }

    /// Visible child controls of `entity`, in order.
    fn child_controls(&self, entity: Entity) -> Vec<Entity> {
        let Some(Children(kids)) = self.children.as_ref().and_then(|c| c.get(entity)) else {
            return Vec::new();
        };
        kids.iter()
            .map(|k| k.entity())
            .filter(|&k| self.controls.contains(k))
            .filter(|&k| {
                self.infos
                    .as_ref()
                    .and_then(|i| i.get(k))
                    .is_some_and(|i| i.visible)
            })
            .collect()
    }

    /// Computes content minimum sizes bottom-up.
    fn measure(&mut self, entity: Entity) {
        let kids = self.child_controls(entity);
        for &kid in &kids {
            self.measure(kid);
        }
        let mut content = Vec2::ZERO;
        let width_hint = self.control(entity).rect.size.x;
        if let Some(label) = self.labels.as_mut().and_then(|l| l.get_mut(entity)) {
            let wrap = label.autowrap.then_some(width_hint.max(1.0));
            let block = label
                .cache
                .get(self.fonts, &label.text, &label.style, wrap, label.h_align);
            content = if label.autowrap {
                Vec2::new(0.0, block.size.y)
            } else {
                block.size
            };
        }
        if let Some(button) = self.buttons.as_mut().and_then(|b| b.get_mut(entity)) {
            let block = button.cache.get(
                self.fonts,
                &button.text,
                &button.text_style,
                None,
                HAlign::Left,
            );
            content = content.max(block.size + button.normal.padding_size());
        }
        if let Some(bar) = self.bars.as_mut().and_then(|b| b.get_mut(entity)) {
            if bar.show_percentage {
                let text = percentage(bar.ratio());
                let block = bar
                    .cache
                    .get(self.fonts, &text, &bar.text_style, None, HAlign::Left);
                content = content.max(Vec2::new(0.0, block.size.y));
            }
        }
        if let Some(panel) = self.panels.as_ref().and_then(|p| p.get(entity)) {
            content = content.max(panel.style.padding_size());
        }
        if let Some(container) = self.boxes.as_ref().and_then(|b| b.get(entity)).copied() {
            let mins: Vec<Vec2> = kids
                .iter()
                .map(|&k| self.control(k).combined_min_size())
                .collect();
            let gaps = container.separation * kids.len().saturating_sub(1) as f32;
            let size = if container.vertical {
                Vec2::new(
                    mins.iter().map(|m| m.x).fold(0.0, f32::max),
                    mins.iter().map(|m| m.y).sum::<f32>() + gaps,
                )
            } else {
                Vec2::new(
                    mins.iter().map(|m| m.x).sum::<f32>() + gaps,
                    mins.iter().map(|m| m.y).fold(0.0, f32::max),
                )
            };
            content = content.max(size);
        }
        self.control_mut(entity).content_min = content;
    }

    /// Assigns `rect` to `entity` and lays out its children.
    fn place(&mut self, entity: Entity, rect: Rect) {
        let rect = Rect::new(rect.position + self.control(entity).translation, rect.size);
        self.control_mut(entity).rect = rect;
        let kids = self.child_controls(entity);
        if kids.is_empty() {
            return;
        }
        let container = self.boxes.as_ref().and_then(|b| b.get(entity)).copied();
        let Some(container) = container else {
            for kid in kids {
                let child_rect = anchored(self.control(kid), &rect);
                self.place(kid, child_rect);
            }
            return;
        };

        let axis = |v: Vec2| if container.vertical { v.y } else { v.x };
        let mins: Vec<f32> = kids
            .iter()
            .map(|&k| axis(self.control(k).combined_min_size()))
            .collect();
        let gaps = container.separation * (kids.len() - 1) as f32;
        let extra = (axis(rect.size) - mins.iter().sum::<f32>() - gaps).max(0.0);
        let expanders = kids.iter().filter(|&&k| self.control(k).expand).count();
        let mut cursor = if expanders > 0 {
            0.0
        } else {
            match container.alignment {
                HAlign::Left => 0.0,
                HAlign::Center => extra * 0.5,
                HAlign::Right => extra,
            }
        };
        for (kid, min) in kids.into_iter().zip(mins) {
            let length = if expanders > 0 && self.control(kid).expand {
                min + extra / expanders as f32
            } else {
                min
            };
            let child_rect = if container.vertical {
                Rect::new(
                    rect.position + Vec2::new(0.0, cursor),
                    Vec2::new(rect.size.x, length),
                )
            } else {
                Rect::new(
                    rect.position + Vec2::new(cursor, 0.0),
                    Vec2::new(length, rect.size.y),
                )
            };
            self.place(kid, child_rect);
            cursor += length + container.separation;
        }
    }

    /// Emits quads for `entity` and its children, back to front.
    fn draw(&mut self, entity: Entity, modulate: Color, clip: Rect) {
        let control = self.control(entity);
        let rect = control.rect;
        let modulate = multiply(modulate, control.modulate);
        let blocks = control.mouse_filter == MouseFilter::Stop;
        let (focusable, clip_contents) = (control.focusable, control.clip_contents);
        let visible_rect = rect.intersect(&clip);
        self.targets.push(HitTarget {
            entity,
            rect: visible_rect,
            blocks,
            focusable,
        });
        let child_clip = if clip_contents { visible_rect } else { clip };

        if let Some(style) = self
            .panels
            .as_ref()
            .and_then(|p| p.get(entity))
            .map(|p| p.style)
        {
            self.style_box(&style, rect, modulate, clip);
        }
        if let Some(image) = self.images.as_ref().and_then(|i| i.get(entity)).copied() {
            self.image(&image, rect, modulate, clip);
        }
        if self.bars.as_ref().is_some_and(|b| b.contains(entity)) {
            self.progress_bar(entity, rect, modulate, clip);
        }
        if self.buttons.as_ref().is_some_and(|b| b.contains(entity)) {
            self.button(entity, rect, modulate, clip);
        }
        if self.labels.as_ref().is_some_and(|l| l.contains(entity)) {
            let label = self
                .labels
                .as_mut()
                .and_then(|l| l.get_mut(entity))
                .expect("checked");
            let wrap = label.autowrap.then_some(rect.size.x.max(1.0));
            let (h_align, v_align, style) = (label.h_align, label.v_align, label.style.clone());
            let block = label
                .cache
                .get(self.fonts, &label.text, &label.style, wrap, h_align)
                .clone();
            self.text(&block, &style, rect, h_align, v_align, modulate, clip);
        }

        for kid in self.child_controls(entity) {
            self.draw(kid, modulate, child_clip);
        }
    }

    fn button(&mut self, entity: Entity, rect: Rect, modulate: Color, clip: Rect) {
        let buttons = self.buttons.as_mut().expect("checked by caller");
        let button = buttons.get_mut(entity).expect("checked by caller");
        let style = *button.current_style();
        let focus = button.focused.then_some(button.focus);
        let text_style = button.text_style.clone();
        let block = button
            .cache
            .get(
                self.fonts,
                &button.text,
                &button.text_style,
                None,
                HAlign::Left,
            )
            .clone();
        let disabled = button.disabled;
        self.style_box(&style, rect, modulate, clip);
        let modulate = if disabled {
            multiply(modulate, Color::rgba(1.0, 1.0, 1.0, 0.5))
        } else {
            modulate
        };
        let content = rect.inset(style.padding);
        self.text(
            &block,
            &text_style,
            content,
            HAlign::Center,
            VAlign::Center,
            modulate,
            clip,
        );
        if let Some(focus) = focus {
            self.style_box(&focus, rect, modulate, clip);
        }
    }

    fn progress_bar(&mut self, entity: Entity, rect: Rect, modulate: Color, clip: Rect) {
        let bars = self.bars.as_mut().expect("checked by caller");
        let bar = bars.get_mut(entity).expect("checked by caller");
        let (background, fill, ratio) = (bar.background, bar.fill, bar.ratio());
        let text = bar.show_percentage.then(|| {
            let text = percentage(ratio);
            let block = bar
                .cache
                .get(self.fonts, &text, &bar.text_style, None, HAlign::Left)
                .clone();
            (block, bar.text_style.clone())
        });
        self.style_box(&background, rect, modulate, clip);
        if ratio > 0.0 {
            let inner = rect.inset([background.border_width; 4]);
            let fill_rect = Rect::new(
                inner.position,
                Vec2::new(inner.size.x * ratio, inner.size.y),
            );
            // Round corners only as far as the fill is wide.
            let fill = StyleBox {
                corner_radius: fill.corner_radius.min(fill_rect.size.x * 0.5),
                ..fill
            };
            self.style_box(&fill, fill_rect, modulate, clip);
        }
        if let Some((block, style)) = text {
            self.text(
                &block,
                &style,
                rect,
                HAlign::Center,
                VAlign::Center,
                modulate,
                clip,
            );
        }
    }

    fn image(&mut self, image: &TextureRect, rect: Rect, modulate: Color, clip: Rect) {
        let Some(texture) = self.assets.and_then(|a| a.texture(image.texture)) else {
            return;
        };
        let texture_size = Vec2::new(texture.width as f32, texture.height as f32);
        let (rect, clip) = match image.stretch {
            StretchMode::Scale => (rect, clip),
            StretchMode::KeepAspectCentered | StretchMode::KeepAspectCovered => {
                let fit = rect.size / texture_size;
                let factor = if image.stretch == StretchMode::KeepAspectCentered {
                    fit.min_element()
                } else {
                    fit.max_element()
                };
                let size = texture_size * factor;
                let fitted = Rect::new(rect.center() - size * 0.5, size);
                (fitted, clip.intersect(&rect))
            }
        };
        let mut quad = UiQuad::solid(self.pixels(rect, true), multiply(image.tint, modulate));
        quad.fill = UiFill::Image(image.texture);
        quad.clip = self.clip_pixels(clip);
        self.quads.push(quad);
    }

    fn style_box(&mut self, style: &StyleBox, rect: Rect, modulate: Color, clip: Rect) {
        if !style.is_visible() || rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }
        let s = self.scale;
        let mut quad = UiQuad::solid(self.pixels(rect, true), multiply(style.color, modulate));
        quad.corner_radius = style.corner_radius * s;
        quad.stroke_width = style.border_width * s;
        quad.stroke_color = multiply(style.border_color, modulate);
        quad.clip = self.clip_pixels(clip);
        self.quads.push(quad);
    }

    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        block: &TextBlock,
        style: &TextStyle,
        rect: Rect,
        h_align: HAlign,
        v_align: VAlign,
        modulate: Color,
        clip: Rect,
    ) {
        let block_width = block
            .lines
            .iter()
            .map(|l| l.x + l.width)
            .fold(0.0, f32::max);
        let offset = Vec2::new(
            match h_align {
                // Lines are already aligned within the block; align the block itself.
                HAlign::Left => 0.0,
                HAlign::Center => (rect.size.x - block_width) * 0.5,
                HAlign::Right => rect.size.x - block_width,
            },
            match v_align {
                VAlign::Top => 0.0,
                VAlign::Center => (rect.size.y - block.size.y) * 0.5,
                VAlign::Bottom => rect.size.y - block.size.y,
            },
        );
        let origin = rect.position + offset;
        let clip = self.clip_pixels(clip);
        let s = self.scale;
        let size_px = style.size * s;
        let px_range = DISTANCE_RANGE * size_px / BAKE_EM_PX;
        // An outline or glow can only extend as far as the stored distance band.
        let max_extent = px_range * 0.5 - 0.5;
        let color = multiply(style.color, modulate);

        let mut passes: Vec<(Vec2, Color, f32, f32, Color)> = Vec::with_capacity(2);
        if let Some(shadow) = style.shadow {
            passes.push((
                shadow.offset * s,
                multiply(shadow.color, modulate),
                (shadow.softness * s).clamp(0.5, max_extent),
                (shadow.spread * s).min(max_extent),
                Color::TRANSPARENT,
            ));
        }
        passes.push((
            Vec2::ZERO,
            color,
            0.0,
            (style.outline_width * s).min(max_extent),
            multiply(style.outline_color, modulate),
        ));

        for (shift, color, softness, stroke, stroke_color) in passes {
            for line in &block.lines {
                // Baselines on whole pixels keep horizontal stems crisp.
                let baseline = ((origin.y + line.baseline) * s).round() + shift.y;
                let line_x = (origin.x + line.x) * s + shift.x;
                for glyph in &line.glyphs {
                    let Some(entry) = self.fonts.glyph(style.font, glyph.glyph) else {
                        continue;
                    };
                    let [left, top, right, bottom] = entry.plane;
                    let x = line_x + glyph.x * s;
                    let y = baseline + glyph.y * s;
                    let [ax, ay, aw, ah] = entry.atlas.map(|v| v as f32);
                    self.quads.push(UiQuad {
                        rect: [
                            x + left * size_px,
                            y + top * size_px,
                            (right - left) * size_px,
                            (bottom - top) * size_px,
                        ],
                        uv: [ax, ay, ax + aw, ay + ah],
                        color,
                        fill: UiFill::Glyph,
                        corner_radius: 0.0,
                        stroke_width: stroke,
                        stroke_color,
                        softness,
                        screen_px_range: px_range,
                        clip,
                    });
                }
            }
        }
    }

    /// UI rect → pixel rect, optionally snapped to whole pixels.
    fn pixels(&self, rect: Rect, snap: bool) -> [f32; 4] {
        let min = rect.min() * self.scale;
        let max = rect.max() * self.scale;
        let (min, max) = if snap {
            (min.round(), max.round())
        } else {
            (min, max)
        };
        [min.x, min.y, max.x - min.x, max.y - min.y]
    }

    fn clip_pixels(&self, clip: Rect) -> [f32; 4] {
        let min = (clip.min() * self.scale).round();
        let max = (clip.max() * self.scale).round();
        [min.x, min.y, max.x, max.y]
    }
}

fn multiply(a: Color, b: Color) -> Color {
    Color::rgba(a.r * b.r, a.g * b.g, a.b * b.b, a.a * b.a)
}

fn percentage(ratio: f32) -> String {
    format!("{}%", (ratio * 100.0).round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_modes() {
        let settings = UiSettings::default();
        assert_eq!(settings.scale(Vec2::new(1920.0, 1080.0)), 1.0);
        assert_eq!(settings.scale(Vec2::new(3840.0, 2160.0)), 2.0);
        // Ultrawide: limited by height.
        assert_eq!(settings.scale(Vec2::new(3440.0, 1440.0)), 1440.0 / 1080.0);
        let width = UiSettings {
            scale_mode: ScaleMode::Width,
            ..settings
        };
        assert_eq!(width.scale(Vec2::new(960.0, 1080.0)), 0.5);
        let fixed = UiSettings {
            scale_mode: ScaleMode::Fixed(0.0),
            ..settings
        };
        assert_eq!(fixed.scale(Vec2::ONE), 1.0, "invalid scale falls back to 1");
    }

    #[test]
    fn anchoring() {
        let parent = Rect::new(Vec2::new(10.0, 20.0), Vec2::new(200.0, 100.0));
        let control = Control::new().with_preset(
            crate::LayoutPreset::BottomRight,
            Vec2::new(50.0, 10.0),
            5.0,
        );
        let rect = anchored(&control, &parent);
        assert_eq!(
            rect,
            Rect::new(Vec2::new(155.0, 105.0), Vec2::new(50.0, 10.0))
        );
        // Grows to the minimum size towards the bottom right.
        let small =
            Control::at(Vec2::ZERO, Vec2::new(5.0, 5.0)).with_min_size(Vec2::new(30.0, 2.0));
        assert_eq!(anchored(&small, &parent).size, Vec2::new(30.0, 5.0));
    }
}
