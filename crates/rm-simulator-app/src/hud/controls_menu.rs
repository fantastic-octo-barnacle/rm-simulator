// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Binding capture consumes input until the activating and captured buttons are released.
use crate::{
    bindings::{Binding, InputAction},
    hud::HudState,
};
use bevy::{
    feathers::{controls::FeathersButton, theme::ThemedText},
    prelude::*,
    ui_widgets::{Activate, ActivateOnPress},
};

/// Owns the binding capture state machine and the message shown on the capture
/// overlay.
#[derive(Resource, Default)]
pub(super) struct RebindState {
    capture: Capture,
    message: String,
}
#[derive(Default)]
enum Capture {
    #[default]
    Idle,
    Waiting {
        action: InputAction,
        slot: usize,
        armed: bool,
    },
    Conflict {
        action: InputAction,
        slot: usize,
        binding: Binding,
        conflicts: Vec<(InputAction, usize)>,
    },
    Release,
}
/// Labels a binding button with the action and the 0-based slot whose current
/// binding it shows, so `sync` can look it up in the controls settings.
#[derive(Component, Clone)]
pub(super) struct BindingLabel(InputAction, usize);
impl Default for BindingLabel {
    fn default() -> Self {
        Self(InputAction::Forward, 0)
    }
}
/// Marks the capture overlay's prompt text, which `sync` fills from the rebind
/// message.
#[derive(Component)]
pub(super) struct CaptureLabel;
/// Marks the invert-mouse caption, which `sync` rewrites from the controls
/// settings.
#[derive(Component, Clone, Default)]
pub(super) struct InvertLabel;
/// Marks the auto-aim target choice caption, which `sync` rewrites from the
/// controls settings.
#[derive(Component, Clone, Default)]
pub(super) struct AimModeLabel;
/// Marks the preferences status line, which `sync` fills from the preference
/// status message.
#[derive(Component)]
pub(super) struct SaveLabel;

/// Adds the Controls tab rows: the invert toggle, one row per input action with
/// two binding buttons and Clear and Default buttons, and the reset-all button.
/// The capture overlay is parented to `shade` so it covers the whole settings
/// card, and the save status line is parented to the same shade.
pub(super) fn spawn(commands: &mut Commands, parent: Entity, shade: Entity) {
    commands.spawn((
        ChildOf(parent),
        Text::new("Controls"),
        TextFont {
            font_size: FontSize::Px(20.),
            ..default()
        },
    ));
    commands.spawn((ChildOf(parent), Text::new("Two bindings per action. Choose one, then press a key or mouse button. Esc cancels.\nAuto Aim and Auto Fire may share a binding. Both default to right mouse. Clear Auto Fire for aim only."), TextFont { font_size: FontSize::Px(13.), ..default() }));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Invert mouse Y: Off") ThemedText InvertLabel } }
        ActivateOnPress
        on(|_: On<Activate>, mut ui: ResMut<HudState>| { ui.controls.invert_y = !ui.controls.invert_y; ui.consumed = true; })
    }).insert(ChildOf(parent));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Auto-aim target: Automatic") ThemedText AimModeLabel } }
        ActivateOnPress
        on(|_: On<Activate>, mut ui: ResMut<HudState>| { ui.controls.manual_aim_mode = !ui.controls.manual_aim_mode; ui.consumed = true; })
    }).insert(ChildOf(parent));
    for action in InputAction::ALL {
        let row = commands
            .spawn((
                ChildOf(parent),
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: px(8.),
                    row_gap: px(4.),
                    align_items: AlignItems::Center,
                    ..default()
                },
            ))
            .id();
        commands.spawn((
            ChildOf(row),
            Text::new(action.label()),
            Node {
                width: px(150.),
                ..default()
            },
            TextFont {
                font_size: FontSize::Px(14.),
                ..default()
            },
        ));
        for slot in 0..2 {
            commands.spawn_scene(bsn! {
                @FeathersButton { @caption: bsn! { Text("Unbound") ThemedText BindingLabel(action, slot) } }
                ActivateOnPress
                on(move |_: On<Activate>, mut capture: ResMut<RebindState>, mut ui: ResMut<HudState>, mut focus: ResMut<bevy::input_focus::InputFocus>| {
                    focus.clear();
                    capture.capture = Capture::Waiting { action, slot, armed: false };
                    capture.message = format!("Release buttons, then choose {} binding. Esc cancels.", action.label());
                    ui.rebinding = true;
                    ui.consumed = true;
                })
            }).insert(ChildOf(row));
            commands.spawn_scene(bsn! {
                @FeathersButton { @caption: bsn! { Text("Clear") ThemedText } }
                ActivateOnPress
                AccessibleLabel(format!("Clear {} binding {}", action.label(), slot + 1))
                on(move |_: On<Activate>, mut ui: ResMut<HudState>, mut capture: ResMut<RebindState>| {
                    ui.controls.set(action, slot, None); ui.consumed = true;
                    capture.capture = Capture::Release;
                })
            }).insert(ChildOf(row));
        }
        commands.spawn_scene(bsn! {
            @FeathersButton { @caption: bsn! { Text("Default") ThemedText } }
            ActivateOnPress
            AccessibleLabel(format!("Reset primary binding for {}", action.label()))
            on(move |_: On<Activate>, mut ui: ResMut<HudState>, mut capture: ResMut<RebindState>| {
                // Reset follows the same conflict flow as capture.
                let binding = crate::bindings::ControlsSettings::default().slots(action)[0].unwrap();
                propose(&mut capture, &mut ui, action, 0, binding);

                ui.consumed = true;
            })
        }).insert(ChildOf(row));
    }
    let overlay = commands
        .spawn((
            ChildOf(shade),
            CaptureOverlay,
            GlobalZIndex(250),
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                width: percent(100.),
                height: percent(100.),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0., 0., 0., 0.75)),
        ))
        .id();
    let prompt = commands
        .spawn((
            ChildOf(overlay),
            Node {
                width: percent(75.),
                max_width: px(650.),
                padding: UiRect::all(px(24.)),
                flex_direction: FlexDirection::Column,
                row_gap: px(16.),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.06, 0.08)),
        ))
        .id();
    commands.spawn((
        ChildOf(prompt),
        Text::new(""),
        CaptureLabel,
        TextFont {
            font_size: FontSize::Px(14.),
            ..default()
        },
    ));
    let row = commands
        .spawn((
            ChildOf(prompt),
            Node {
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(10.),
                row_gap: px(4.),
                ..default()
            },
        ))
        .id();
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Replace conflicting bindings") ThemedText } }
        ActivateOnPress
        on(|_: On<Activate>, mut capture: ResMut<RebindState>, mut ui: ResMut<HudState>| {
            if let Capture::Conflict { action, slot, binding, conflicts } = std::mem::take(&mut capture.capture) {
                for (other, index) in conflicts { ui.controls.set(other, index, None); }
                ui.controls.set(action, slot, Some(binding));
                capture.message = format!("{}: {}", action.label(), binding.label());
            }
            capture.capture = Capture::Release; ui.rebinding = true; ui.consumed = true;
        })
    }).insert((ChildOf(row), ConflictButton));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Cancel rebind") ThemedText } }
        ActivateOnPress
        on(|_: On<Activate>, mut capture: ResMut<RebindState>, mut ui: ResMut<HudState>| {
            capture.capture = Capture::Release; capture.message = "Binding unchanged".into(); ui.rebinding = true; ui.consumed = true;
        })
    }).insert(ChildOf(row));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Reset all controls") ThemedText } }
        ActivateOnPress
        on(|_: On<Activate>, mut capture: ResMut<RebindState>, mut ui: ResMut<HudState>| {
            ui.controls = default(); ui.sensitivity = 0.0025; ui.rebinding = true; ui.consumed = true;
            capture.capture = Capture::Release; capture.message = "Default controls restored".into();
        })
    }).insert(ChildOf(parent));
    commands.spawn((
        ChildOf(shade),
        Node {
            position_type: PositionType::Absolute,
            bottom: px(8.),
            ..default()
        },
        Text::new(""),
        SaveLabel,
        TextFont {
            font_size: FontSize::Px(13.),
            ..default()
        },
    ));
}
fn propose(
    capture: &mut RebindState,
    ui: &mut HudState,
    action: InputAction,
    slot: usize,
    binding: Binding,
) {
    let conflicts = ui.controls.conflicts(action, slot, binding);
    if conflicts.is_empty() {
        ui.controls.set(action, slot, Some(binding));
        capture.message = format!("{}: {}", action.label(), binding.label());
        capture.capture = Capture::Release;
    } else {
        capture.message = format!(
            "{} is already used by {}. Replace those bindings or cancel.",
            binding.label(),
            conflicts
                .iter()
                .map(|(action, _)| action.label())
                .collect::<Vec<_>>()
                .join(", ")
        );
        capture.capture = Capture::Conflict {
            action,
            slot,
            binding,
            conflicts,
        };
    }
    ui.rebinding = true;
    ui.consumed = true;
}
/// Consumes keys and mouse buttons while a rebind is in progress.
/// Waits for every held button to be released before recording the next press,
/// treats Escape as cancel, and keeps input consumed so a captured key cannot
/// drive, aim or fire. Abandons a pending rebind when the settings panel closes.
/// Runs before `panel_input`, so a captured key never reaches a panel shortcut.
pub(super) fn capture_input(
    mut capture: ResMut<RebindState>,
    mut ui: ResMut<HudState>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
) {
    let released = keys.get_pressed().next().is_none() && buttons.get_pressed().next().is_none();
    if !ui.settings && !matches!(capture.capture, Capture::Idle) {
        capture.capture = Capture::Release;
    }
    match capture.capture {
        Capture::Waiting {
            action,
            slot,
            armed,
        } => {
            ui.rebinding = true;
            if !armed {
                if released {
                    capture.capture = Capture::Waiting {
                        action,
                        slot,
                        armed: true,
                    };
                    capture.message = format!(
                        "Press a key or mouse button for {}. Esc cancels.",
                        action.label()
                    );
                }
                return;
            }
            if keys.just_pressed(KeyCode::Escape) {
                capture.capture = Capture::Release;
                capture.message = "Binding unchanged".into();
                return;
            }
            let binding = keys
                .get_just_pressed()
                .next()
                .copied()
                .map(Binding::Key)
                .or_else(|| {
                    buttons
                        .get_just_pressed()
                        .next()
                        .copied()
                        .map(Binding::Mouse)
                });
            if let Some(binding) = binding {
                propose(&mut capture, &mut ui, action, slot, binding);
            }
        }
        Capture::Conflict { .. } => {
            ui.rebinding = true;
            if keys.just_pressed(KeyCode::Escape) {
                capture.capture = Capture::Release;
                capture.message = "Binding unchanged".into();
            }
        }
        Capture::Release => {
            ui.rebinding = true;
            ui.consumed = true;
            if released {
                capture.capture = Capture::Idle;
                ui.rebinding = false;
            }
        }
        Capture::Idle => ui.rebinding = false,
    }
}
/// Rewrites each labelled caption from the controls settings, the capture
/// message, the invert flag or the preference status, and leaves unlabelled
/// text alone.
#[allow(clippy::type_complexity)]
pub(super) fn sync(
    ui: Res<HudState>,
    capture: Res<RebindState>,
    status: Option<Res<crate::preferences::PreferenceStatus>>,
    mut labels: Query<(
        &mut Text,
        Option<&BindingLabel>,
        Has<CaptureLabel>,
        Has<InvertLabel>,
        Has<AimModeLabel>,
        Has<SaveLabel>,
    )>,
) {
    for (mut text, binding, capturing, invert, aim_mode, save) in &mut labels {
        let next = if let Some(binding) = binding {
            ui.controls.slots(binding.0)[binding.1]
                .map(Binding::label)
                .unwrap_or_else(|| "Unbound".into())
        } else if capturing {
            capture.message.clone()
        } else if invert {
            format!(
                "Invert mouse Y: {}",
                if ui.controls.invert_y { "On" } else { "Off" }
            )
        } else if aim_mode {
            if ui.controls.manual_aim_mode {
                format!(
                    "Auto-aim target: Manual ({} switches)",
                    ui.controls.label(InputAction::AimMode)
                )
            } else {
                "Auto-aim target: Automatic".into()
            }
        } else if save {
            status
                .as_ref()
                .map(|s| s.message.clone())
                .unwrap_or_default()
        } else {
            continue;
        };
        if text.0 != next {
            text.0 = next;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_waits_for_release_and_conflicts_require_explicit_replacement() {
        let mut app = App::new();
        app.init_resource::<HudState>()
            .init_resource::<RebindState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(Update, capture_input);
        app.world_mut().resource_mut::<HudState>().settings = true;
        app.world_mut().resource_mut::<RebindState>().capture = Capture::Waiting {
            action: InputAction::Fire,
            slot: 0,
            armed: false,
        };
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        assert!(matches!(
            app.world().resource::<RebindState>().capture,
            Capture::Waiting { armed: false, .. }
        ));
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.update();
        assert!(matches!(
            app.world().resource::<RebindState>().capture,
            Capture::Conflict { .. }
        ));
        assert_eq!(
            app.world()
                .resource::<HudState>()
                .controls
                .slots(InputAction::Fire)[0],
            Some(Binding::Mouse(MouseButton::Left))
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert!(app.world().resource::<HudState>().blocks_input());
        assert!(app.world().resource::<HudState>().settings);
    }
}

/// Marks the modal overlay shown while a rebind waits for a press or reports a
/// conflict.
#[derive(Component)]
pub(super) struct CaptureOverlay;
/// Marks the button that replaces the existing bindings conflicting with the
/// captured one.
#[derive(Component)]
pub(super) struct ConflictButton;
/// Shows the capture overlay while a rebind is waiting or in conflict, and the
/// conflict button only for a conflict.
pub(super) fn sync_overlay(
    capture: Res<RebindState>,
    mut overlays: Query<&mut Node, With<CaptureOverlay>>,
    mut conflicts: Query<&mut Node, (With<ConflictButton>, Without<CaptureOverlay>)>,
) {
    let display = if matches!(
        capture.capture,
        Capture::Waiting { .. } | Capture::Conflict { .. }
    ) {
        Display::Flex
    } else {
        Display::None
    };
    let conflict_display = if matches!(capture.capture, Capture::Conflict { .. }) {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in &mut conflicts {
        if node.display != conflict_display {
            node.display = conflict_display;
        }
    }
    for mut node in &mut overlays {
        if node.display != display {
            node.display = display;
        }
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::*;
    #[test]
    fn replacement_button_is_shown_only_for_a_conflict() {
        let mut app = App::new();
        app.init_resource::<RebindState>()
            .add_systems(Update, sync_overlay);
        let overlay = app
            .world_mut()
            .spawn((CaptureOverlay, Node::default()))
            .id();
        let button = app
            .world_mut()
            .spawn((ConflictButton, Node::default()))
            .id();
        app.world_mut().resource_mut::<RebindState>().capture = Capture::Waiting {
            action: InputAction::Forward,
            slot: 0,
            armed: true,
        };
        app.update();
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(button).unwrap().display,
            Display::None
        );
        app.world_mut().resource_mut::<RebindState>().capture = Capture::Conflict {
            action: InputAction::Forward,
            slot: 0,
            binding: Binding::Key(KeyCode::KeyS),
            conflicts: vec![(InputAction::Backward, 0)],
        };
        app.update();
        assert_eq!(
            app.world().get::<Node>(button).unwrap().display,
            Display::Flex
        );
    }
}
