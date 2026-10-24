# April Blink Engine

April Blink Engine is a lightweight 3D game engine inspired by Godot, written from scratch in
Rust. It deliberately leaves features out: the goal is a simpler, focused and efficient
foundation for making games, with a small codebase that one person can read and change.

![The April Blink editor with the Night Relics level open](.github/screenshots/editor.png)

<table>
  <tr>
    <td width="50%"><img src=".github/screenshots/demo.png" alt="The demo: a ruin at night with physics crates, a ragdoll and bilingual UI"></td>
    <td width="50%"><img src=".github/screenshots/night-relics.png" alt="Night Relics, the vertical slice"></td>
  </tr>
  <tr>
    <td align="center"><b>The demo</b>: physics, lights, particles, UI</td>
    <td align="center"><b>Night Relics</b>: the vertical slice</td>
  </tr>
</table>

![Night Relics playing inside the editor](.github/screenshots/play-in-editor.png)
<p align="center"><i>Play in editor: the game runs in the viewport while the scene tree stays live.</i></p>

## Features

- **Scenes**: a node tree (`Node3D`, physics bodies, lights, cameras, UI controls…) on a
  small sparse-set ECS, with signals, groups, tweens and `.abscene` scene files (TOML
  inside).
- **Rendering**: a wgpu forward renderer with shadows, bloom, skinned meshes, particles,
  debug lines and MSDF text (complex scripts such as Thai included).
- **Physics**: Rapier 3D bodies, joints, ragdolls, areas, scene queries and a kinematic
  character controller; grid navigation baked from the physics world.
- **Audio**: Kira streams, 2D/3D players and a listener.
- **Languages**: translations in `locales/<code>.toml` (`ab-locale`) with fallbacks and
  `{placeholders}`; UI labels and buttons with a `key` follow the language at runtime, and
  fonts fall back per character for scripts the UI font lacks.
- **Prefabs and materials**: any scene file is a prefab; instances keep their own changes
  as overrides (a changed prefab reaches every instance, overrides still win; a missing one
  loads as a placeholder that saves back unchanged). Materials are files too (`.abmat`)
  with albedo, normal and metal/rough maps.
- **Config**: named values in `config.toml` (`Player.MoveSpeed`, `Game.DefaultLanguage`)
  with types, ranges, options and constants, read and changed safely by scripts.
- **Editor**: scene editing, play in editor, assets, inspector, console, profiler, source
  control and one-click export; right-click menus, cut/copy/paste of objects, configurable
  snapping and shortcuts from one command table (F1 lists them), four views (perspective,
  top, front, side), English and Thai with a font and scale of your choice (Settings →
  Preferences), thumbnails and a viewer for images, models, scenes and materials. The
  main window keeps the panels used all the time (scene, inspector, environment, assets,
  console); each feature has a window of its own: double-click a UI node for the UI
  Editor (one interface at a time), an AnimationPlayer for the Animation Editor, an
  effect for the Effects Editor; Settings → Game Config; Window → Level Builder (levels
  from pieces on a grid: stack, turn, align, distribute, rows), Profiler, Debug and
  source control. The engine sources are found again when the checkout moves, or
  located once from the loading screen.
- **Scripting**: Rust scripts built against the `ab-script` SDK and loaded through a stable
  C ABI, so the editor and games never need rebuilding with them; rebuilt and hot reloaded
  while the game or the editor runs.
- **Shipping**: a prebuilt player plus the game's script library and an `assets/` folder of
  chunks named by hash (one `.abp` pack per top-level asset folder, plus an index), so a
  patch ships only the chunks that changed; or one pack inside the executable. For Linux,
  Windows and macOS (a signed `.app` bundle, exported on a Mac); the editor itself ships as
  a standalone download.

## Requirements

- Rust **1.95** or newer (`rustup update stable`).
- A GPU driver with Vulkan, DX12 or Metal.
- Linux: `libwayland-dev libxkbcommon-dev libudev-dev libasound2-dev` (Debian/Ubuntu names).
- Windows builds from Linux (optional): `cargo install --locked cargo-xwin`, plus `clang` and
  `lld`.
- macOS: the Xcode command line tools (`xcode-select --install`).

## Workspace

| Crate | Purpose |
| --- | --- |
| `ab-core` | Math (glam), time, identifiers, logging, job pool |
| `ab-ecs` | Minimal sparse-set ECS under the node API |
| `ab-scene` | Godot-style node tree, signals, scene files |
| `ab-platform` | Windowing, OS events, frame pacing (winit), gamepads (gilrs) |
| `ab-input` | Keyboard, mouse and gamepad state with an action map |
| `ab-asset` | glTF 2.0 and PNG/JPEG loading and caching |
| `ab-vfs` | Virtual file system, `.abp` asset packs and chunk folders |
| `ab-rendering` | wgpu forward renderer |
| `ab-animation` | Keyframe clips and playback |
| `ab-physics` | Rapier 3D physics, joints, ragdolls, character controller |
| `ab-navigation` | Grid navigation baked from physics, A* paths |
| `ab-effects` | Particles, one-shot effects, light flashes, camera shake |
| `ab-audio` | Kira audio: streams, 2D/3D players, listener |
| `ab-ui` | Screen-space controls, layout and MSDF text |
| `ab-locale` | Language files, fallbacks and placeholders (editor and games) |
| `ab-engine` | Facade: game loop, scheduling, subsystem wiring, script host, hot reload |
| `ab-script-api` | The C ABI between the engine and script libraries |
| `ab-script` | The script SDK game projects depend on |
| `ab-project` | Projects: template, script builds, export, archives |
| `ab-player` | The prebuilt executable exported games run on |
| `ab-editor` | The editor |
| `ab-app`, `ab-demo-scripts` | The demo and its scripts |
| `ab-slice` | Night Relics, the vertical slice |
| `tools/ab-pack` | Creates, inspects and verifies `.abp` packs and chunk folders |
| `xtask` | Project automation (`cargo xtask`) |

## Main dependencies

| Library | Version |
| --- | --- |
| wgpu | 30 |
| winit | 0.30 |
| glam | 0.33 (shared with Rapier and Kira) |
| rapier3d | 0.36 |
| kira | 0.12 |
| eframe / egui | 0.36 (egui_dock 0.21) |
| gltf | 1.4 |
| image | 0.25 |

## License

MIT. See [LICENSE](LICENSE).
