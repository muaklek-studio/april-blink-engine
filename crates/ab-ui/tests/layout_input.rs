//! Layout and interaction without a GPU: rects, containers, visibility, hover, clicks and
//! keyboard/gamepad focus.

use ab_core::{Color, Time, Vec2};
use ab_input::{GamepadButton, InputState, KeyCode, MouseButton};
use ab_platform::{GamepadId, PlatformEvent};
use ab_rendering::{UiDrawList, UiFill};
use ab_scene::{NodeHandle, SceneTree};
use ab_ui::{
    prepare_ui, ui_animation_system, ui_input_system, BoxContainer, Button, Control, HAlign, Label,
    LayoutPreset, Panel, Rect, StyleBox, TextStyle, UiEvent, UiNodes, UiState,
};

fn ui_tree() -> SceneTree {
    let mut tree = SceneTree::new();
    ab_ui::register(tree.world_mut());
    tree.world_mut().insert_resource(InputState::new());
    tree
}

fn rect(tree: &SceneTree, node: NodeHandle) -> Rect {
    tree.component::<Control>(node).unwrap().rect()
}

/// One frame: input (against last frame's layout), then layout for 1920×1080.
fn frame(tree: &mut SceneTree, events: &[PlatformEvent]) {
    {
        let world = tree.world();
        let mut input = world.resource_mut::<InputState>().unwrap();
        for event in events {
            input.handle_event(event);
        }
    }
    ui_input_system(tree.world_mut());
    prepare_ui(tree.world(), 1920, 1080);
    tree.world()
        .resource_mut::<InputState>()
        .unwrap()
        .end_frame();
}

fn events(tree: &SceneTree) -> Vec<UiEvent> {
    tree.world()
        .resource::<UiState>()
        .unwrap()
        .events()
        .to_vec()
}

fn cursor(x: f64, y: f64) -> PlatformEvent {
    PlatformEvent::CursorMoved { x, y }
}

fn mouse(pressed: bool) -> PlatformEvent {
    PlatformEvent::MouseButton {
        button: MouseButton::Left,
        pressed,
    }
}

fn key(code: KeyCode, pressed: bool) -> PlatformEvent {
    PlatformEvent::Key {
        code,
        pressed,
        repeat: false,
    }
}

/// A centered menu: VBox with a title and three buttons, the middle one expanding.
struct Menu {
    panel: NodeHandle,
    column: NodeHandle,
    title: NodeHandle,
    buttons: [NodeHandle; 3],
}

fn menu(tree: &mut SceneTree) -> Menu {
    let layer = tree.create_canvas_layer("Menu", 10);
    tree.add_child(tree.root(), layer).unwrap();
    let panel = tree.create_panel(
        "Panel",
        Control::new().with_preset(LayoutPreset::Center, Vec2::new(400.0, 400.0), 0.0),
        Panel::default(),
    );
    tree.add_child(layer, panel).unwrap();
    let column = tree.create_box_container(
        "Column",
        Control::full_rect().with_preset(LayoutPreset::FullRect, Vec2::ZERO, 20.0),
        BoxContainer::vertical().with_separation(10.0),
    );
    tree.add_child(panel, column).unwrap();
    let title = tree.create_label(
        "Title",
        Control::new(),
        Label::new("Paused").with_style(TextStyle::new(40.0)),
    );
    tree.add_child(column, title).unwrap();
    let mut buttons = [NodeHandle::from_bits(0); 3];
    for (i, text) in ["Resume", "Options", "Quit"].into_iter().enumerate() {
        let control = Control::new()
            .with_min_size(Vec2::new(0.0, 60.0))
            .with_expand(i == 1);
        buttons[i] = tree.create_button(text, control, Button::new(text));
        tree.add_child(column, buttons[i]).unwrap();
    }
    Menu {
        panel,
        column,
        title,
        buttons,
    }
}

#[test]
fn box_container_stacks_and_expands() {
    let mut tree = ui_tree();
    let menu = menu(&mut tree);
    frame(&mut tree, &[]);

    assert_eq!(
        rect(&tree, menu.panel),
        Rect::new(Vec2::new(760.0, 340.0), Vec2::new(400.0, 400.0))
    );
    let column = rect(&tree, menu.column);
    assert_eq!(
        column,
        Rect::new(Vec2::new(780.0, 360.0), Vec2::new(360.0, 360.0))
    );

    let title = rect(&tree, menu.title);
    let [a, b, c] = menu.buttons.map(|n| rect(&tree, n));
    // Children fill the width, stack top to bottom with the separation.
    for r in [title, a, b, c] {
        assert_eq!(r.position.x, column.position.x);
        assert_eq!(r.size.x, column.size.x);
    }
    assert_eq!(title.position.y, column.position.y);
    assert!(title.size.y > 40.0, "label height from the font metrics");
    assert_eq!(a.position.y, title.max().y + 10.0);
    assert_eq!(a.size.y, 60.0);
    assert_eq!(b.position.y, a.max().y + 10.0);
    // The expanding button takes all leftover space; the last one ends at the bottom.
    assert!(b.size.y > 60.0);
    assert!((c.max().y - column.max().y).abs() < 1e-3);
}

#[test]
fn hidden_controls_take_no_space_and_draw_nothing() {
    let mut tree = ui_tree();
    let menu = menu(&mut tree);
    frame(&mut tree, &[]);
    let quads_before = tree.world().resource::<UiDrawList>().unwrap().quads.len();
    tree.set_visible(menu.title, false).unwrap();
    frame(&mut tree, &[]);
    let column = rect(&tree, menu.column);
    assert_eq!(rect(&tree, menu.buttons[0]).position.y, column.position.y);
    let quads_after = tree.world().resource::<UiDrawList>().unwrap().quads.len();
    assert!(quads_after < quads_before);

    // Hiding a canvas layer hides everything under it.
    let layer = tree.parent(menu.panel).unwrap();
    tree.set_visible(layer, false).unwrap();
    frame(&mut tree, &[]);
    assert!(tree
        .world()
        .resource::<UiDrawList>()
        .unwrap()
        .quads
        .is_empty());
}

#[test]
fn layers_draw_in_order() {
    let mut tree = ui_tree();
    for (name, layer, color) in [("Top", 5, Color::RED), ("Bottom", -1, Color::BLUE)] {
        let canvas = tree.create_canvas_layer(name, layer);
        tree.add_child(tree.root(), canvas).unwrap();
        let panel = tree.create_panel(
            "Box",
            Control::full_rect(),
            Panel {
                style: StyleBox::flat(color),
            },
        );
        tree.add_child(canvas, panel).unwrap();
    }
    frame(&mut tree, &[]);
    let list = tree.world().resource::<UiDrawList>().unwrap();
    let colors: Vec<Color> = list.quads.iter().map(|q| q.color).collect();
    assert_eq!(colors, [Color::BLUE, Color::RED]);
    assert!(list.atlas.is_none(), "no text, no atlas upload");
}

#[test]
fn clicking_a_button_fires_on_release_over_it() {
    let mut tree = ui_tree();
    let menu = menu(&mut tree);
    frame(&mut tree, &[]);
    let resume = menu.buttons[0];
    let center = rect(&tree, resume).center();
    let pressed = |tree: &SceneTree| tree.component::<Button>(resume).unwrap().just_pressed();

    frame(&mut tree, &[cursor(center.x as f64, center.y as f64)]);
    let state = tree.world().resource::<UiState>().unwrap().clone();
    assert_eq!(state.hovered(), Some(resume));
    assert!(state.is_pointer_over_ui());
    assert!(tree.component::<Button>(resume).unwrap().is_hovered());

    frame(&mut tree, &[mouse(true)]);
    assert!(tree.component::<Button>(resume).unwrap().is_held());
    assert!(!pressed(&tree), "fires on release");
    assert_eq!(events(&tree), [UiEvent::FocusChanged(Some(resume))]);

    frame(&mut tree, &[mouse(false)]);
    assert!(pressed(&tree));
    assert_eq!(events(&tree), [UiEvent::Pressed(resume)]);
    assert!(tree
        .world()
        .resource::<UiState>()
        .unwrap()
        .was_pressed(resume));
    frame(&mut tree, &[]);
    assert!(!pressed(&tree), "only for one frame");

    // Press, drag off, release: no click.
    frame(&mut tree, &[mouse(true)]);
    frame(&mut tree, &[cursor(10.0, 10.0)]);
    assert!(!tree
        .world()
        .resource::<UiState>()
        .unwrap()
        .is_pointer_over_ui());
    frame(&mut tree, &[mouse(false)]);
    assert!(!pressed(&tree));

    // Disabled buttons ignore clicks; labels let the pointer through to the panel.
    tree.component_mut::<Button>(resume).unwrap().disabled = true;
    frame(
        &mut tree,
        &[cursor(center.x as f64, center.y as f64), mouse(true)],
    );
    frame(&mut tree, &[mouse(false)]);
    assert!(!pressed(&tree));
    let title = rect(&tree, menu.title).center();
    frame(&mut tree, &[cursor(title.x as f64, title.y as f64)]);
    assert_eq!(
        tree.world().resource::<UiState>().unwrap().hovered(),
        Some(menu.panel)
    );
}

#[test]
fn pointer_maps_through_the_ui_scale() {
    let mut tree = ui_tree();
    let menu = menu(&mut tree);
    // Lay out at 4K: UI units stay the same, pixels double.
    prepare_ui(tree.world(), 3840, 2160);
    let center = rect(&tree, menu.buttons[2]).center() * 2.0;
    {
        let world = tree.world();
        let mut input = world.resource_mut::<InputState>().unwrap();
        input.handle_event(&cursor(center.x as f64, center.y as f64));
    }
    ui_input_system(tree.world_mut());
    assert_eq!(
        tree.world().resource::<UiState>().unwrap().hovered(),
        Some(menu.buttons[2])
    );
}

#[test]
fn keyboard_and_gamepad_focus_navigation() {
    let mut tree = ui_tree();
    let menu = menu(&mut tree);
    frame(&mut tree, &[]);
    let focus = |tree: &SceneTree| tree.world().resource::<UiState>().unwrap().focused();

    // Arrows do nothing until something has focus (they belong to the game).
    frame(&mut tree, &[key(KeyCode::ArrowDown, true)]);
    assert_eq!(focus(&tree), None);
    frame(
        &mut tree,
        &[key(KeyCode::ArrowDown, false), key(KeyCode::Tab, true)],
    );
    assert_eq!(focus(&tree), Some(menu.buttons[0]));
    assert!(tree
        .component::<Button>(menu.buttons[0])
        .unwrap()
        .has_focus());
    frame(
        &mut tree,
        &[key(KeyCode::Tab, false), key(KeyCode::ArrowDown, true)],
    );
    assert_eq!(focus(&tree), Some(menu.buttons[1]));
    // Shift+Tab goes back; wrapping around.
    frame(
        &mut tree,
        &[
            key(KeyCode::ArrowDown, false),
            key(KeyCode::ShiftLeft, true),
            key(KeyCode::Tab, true),
        ],
    );
    assert_eq!(focus(&tree), Some(menu.buttons[0]));
    frame(
        &mut tree,
        &[key(KeyCode::Tab, false), key(KeyCode::Tab, true)],
    );
    assert_eq!(focus(&tree), Some(menu.buttons[2]));
    frame(
        &mut tree,
        &[key(KeyCode::Tab, false), key(KeyCode::ShiftLeft, false)],
    );

    // Disabled buttons are skipped.
    tree.component_mut::<Button>(menu.buttons[0])
        .unwrap()
        .disabled = true;
    frame(&mut tree, &[key(KeyCode::Tab, true)]);
    assert_eq!(focus(&tree), Some(menu.buttons[1]));
    tree.component_mut::<Button>(menu.buttons[0])
        .unwrap()
        .disabled = false;

    // Enter activates the focused button.
    frame(
        &mut tree,
        &[key(KeyCode::Tab, false), key(KeyCode::Enter, true)],
    );
    assert_eq!(events(&tree), [UiEvent::Pressed(menu.buttons[1])]);

    // Gamepad: D-pad moves, A activates.
    let pad = GamepadId(0);
    let button = |button, pressed| PlatformEvent::GamepadButton {
        id: pad,
        button,
        pressed,
    };
    frame(
        &mut tree,
        &[
            key(KeyCode::Enter, false),
            PlatformEvent::GamepadConnected { id: pad },
            button(GamepadButton::DPadDown, true),
        ],
    );
    assert_eq!(focus(&tree), Some(menu.buttons[2]));
    frame(
        &mut tree,
        &[
            button(GamepadButton::DPadDown, false),
            button(GamepadButton::South, true),
        ],
    );
    assert_eq!(events(&tree), [UiEvent::Pressed(menu.buttons[2])]);

    // Escape drops focus; so does clicking outside the UI.
    frame(&mut tree, &[key(KeyCode::Escape, true)]);
    assert_eq!(focus(&tree), None);
    assert_eq!(events(&tree), [UiEvent::FocusChanged(None)]);
    frame(
        &mut tree,
        &[key(KeyCode::Escape, false), key(KeyCode::Tab, true)],
    );
    assert!(focus(&tree).is_some());
    frame(
        &mut tree,
        &[key(KeyCode::Tab, false), cursor(5.0, 5.0), mouse(true)],
    );
    assert_eq!(focus(&tree), None);

    // A freed focused control loses focus.
    frame(&mut tree, &[mouse(false), key(KeyCode::Tab, true)]);
    let focused = focus(&tree).unwrap();
    tree.queue_free(focused).unwrap();
    tree.process(0.0);
    frame(&mut tree, &[key(KeyCode::Tab, false)]);
    assert_eq!(focus(&tree), None);
}

#[test]
fn clip_contents_and_modulate() {
    let mut tree = ui_tree();
    let mut clipper = Control::at(Vec2::new(100.0, 100.0), Vec2::new(50.0, 50.0));
    clipper.clip_contents = true;
    clipper.modulate = Color::rgba(1.0, 1.0, 1.0, 0.5);
    let clipper = tree.create_control("Clip", clipper);
    tree.add_child(tree.root(), clipper).unwrap();
    let label = tree.create_label(
        "Long",
        Control::new(),
        Label::new("This text is much wider than its parent").with_style(TextStyle::new(30.0)),
    );
    tree.add_child(clipper, label).unwrap();
    frame(&mut tree, &[]);
    let list = tree.world().resource::<UiDrawList>().unwrap();
    assert!(list.atlas.is_some());
    let glyphs: Vec<_> = list
        .quads
        .iter()
        .filter(|q| q.fill == UiFill::Glyph)
        .collect();
    assert!(!glyphs.is_empty());
    for glyph in glyphs {
        assert_eq!(glyph.clip, [100.0, 100.0, 150.0, 150.0]);
        assert!((glyph.color.a - 0.5).abs() < 1e-6, "parent alpha applies");
    }
}

#[test]
fn animations_fade_and_slide() {
    let mut tree = ui_tree();
    let mut time = Time::new(1.0 / 60.0);
    time.begin_frame(0.1);
    tree.world_mut().insert_resource(time);
    let toast = tree.create_label(
        "Toast",
        Control::new().with_preset(LayoutPreset::CenterTop, Vec2::new(300.0, 50.0), 20.0),
        Label::new("Saved").with_align(HAlign::Center, Default::default()),
    );
    tree.add_child(tree.root(), toast).unwrap();
    tree.animate(toast)
        .unwrap()
        .fade(0.0, 1.0, 0.5)
        .slide_in(Vec2::new(0.0, -80.0), 0.5);

    ui_animation_system(tree.world_mut());
    prepare_ui(tree.world(), 1920, 1080);
    let control = tree.component::<Control>(toast).unwrap().clone();
    assert!(control.modulate.a > 0.0 && control.modulate.a < 1.0);
    assert!(control.translation.y < 0.0);
    assert!(
        control.rect().position.y < 20.0,
        "drawn above its resting place"
    );

    for _ in 0..5 {
        ui_animation_system(tree.world_mut());
    }
    prepare_ui(tree.world(), 1920, 1080);
    let control = tree.component::<Control>(toast).unwrap().clone();
    assert_eq!(control.modulate.a, 1.0);
    assert_eq!(control.rect().position.y, 20.0);
    assert!(!tree.animate(toast).unwrap().is_animating());
    // Non-controls cannot be animated.
    let plain = tree.create_node("Plain");
    assert!(tree.animate(plain).is_none());
}

#[test]
fn buttons_emit_the_pressed_signal() {
    use ab_scene::{NodeScript, Signal};

    #[derive(Default)]
    struct Counter(u32);
    impl NodeScript for Counter {
        fn on_signal(&mut self, _: &mut SceneTree, _: NodeHandle, signal: &Signal) {
            if signal.is(Button::PRESSED) {
                self.0 += 1;
            }
        }
    }

    let mut tree = ui_tree();
    let menu = menu(&mut tree);
    let counter = tree.create_node("Counter");
    tree.set_script(counter, Counter::default()).unwrap();
    tree.add_child(tree.root(), counter).unwrap();
    tree.connect(menu.buttons[2], Button::PRESSED, counter);
    frame(&mut tree, &[]);
    let center = rect(&tree, menu.buttons[2]).center();
    frame(
        &mut tree,
        &[cursor(center.x as f64, center.y as f64), mouse(true)],
    );
    frame(&mut tree, &[mouse(false)]);
    tree.process(0.0); // delivers queued signals
    assert_eq!(tree.script::<Counter>(counter).unwrap().0, 1);
}
