// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Named client actions. Physical bindings never cross the simulation protocol.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A named control the player can bind. Every input the app reads is looked up
/// through one of these, so rebinding never changes what a command means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputAction {
    /// Drive or fly forward.
    Forward,
    /// Drive or fly backward.
    Backward,
    /// Drive or fly left.
    Left,
    /// Drive or fly right.
    Right,
    /// Fly up. Only the free camera has a vertical axis.
    Up,
    /// Fly down. Only the free camera has a vertical axis.
    Down,
    /// Hold to move faster: 5 m/s driving, 8 m/s flying.
    Fast,
    /// Toggle spin mode; the pilot's chassis is the only thing that spins.
    Spin,
    /// Toggle the first- and third-person camera; only a chassis has an eye on it.
    Camera,
    /// Hold to shoot at the gun's cadence: a chassis gun through the host's
    /// weapon check, a free camera's injection otherwise.
    Fire,
    /// Hold to steer the gun at an auto-aim target.
    AutoAim,
    /// Hold to let auto aim fire when its solution is clear, on the same cadence
    /// as a manual trigger.
    AutoFire,
    /// Switch the auto-aim target class between armor and rune while
    /// [`ControlsSettings::manual_aim_mode`] is on; does nothing otherwise.
    AimMode,
    /// Buy one 17 mm round for the pilot's chassis from team gold. A client with
    /// no chassis, such as a spectator or the referee, sends nothing.
    Buy17,
    /// Buy one 42 mm round for the pilot's chassis from team gold. A client with
    /// no chassis, such as a spectator or the referee, sends nothing.
    Buy42,
    /// Toggle the settings panel.
    Settings,
    /// Toggle the large map panel.
    Map,
    /// Toggle the debug panel.
    Debug,
    /// Hold to show the controls help panel.
    Help,
    /// Hold to show the team roster panel.
    Roster,
    /// Cycle the physics collider view: hidden, overlay, then alone.
    Colliders,
    /// Pause or resume the match clock. A remote host lets only the referee do
    /// this; anyone whose world is local runs the match themselves.
    Pause,
    /// Advance a paused world by 16 ms. A remote host lets only the referee do
    /// this.
    Step,
    /// Start an idle match, or reset a finished one. A remote host lets only the
    /// referee do this.
    Start,
    /// Spend one opportunity to activate this session's team rune. A remote host
    /// lets only the referee do this, and only while the round runs.
    Rune,
}
impl InputAction {
    /// Every action, in the order the help panel and the controls menu list them.
    pub const ALL: [Self; 25] = [
        Self::Forward,
        Self::Backward,
        Self::Left,
        Self::Right,
        Self::Up,
        Self::Down,
        Self::Fast,
        Self::Spin,
        Self::Camera,
        Self::Fire,
        Self::AutoAim,
        Self::AutoFire,
        Self::AimMode,
        Self::Buy17,
        Self::Buy42,
        Self::Settings,
        Self::Map,
        Self::Debug,
        Self::Help,
        Self::Roster,
        Self::Colliders,
        Self::Pause,
        Self::Step,
        Self::Start,
        Self::Rune,
    ];
    /// The action's user-facing name, as menus and messages show it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Forward => "Move forward",
            Self::Backward => "Move backward",
            Self::Left => "Move left",
            Self::Right => "Move right",
            Self::Up => "Fly up",
            Self::Down => "Fly down",
            Self::Fast => "Move faster",
            Self::Spin => "Toggle spin",
            Self::Camera => "Change camera",
            Self::Fire => "Fire",
            Self::AutoAim => "Hold auto aim",
            Self::AutoFire => "Hold auto fire",
            Self::AimMode => "Switch auto-aim mode",
            Self::Buy17 => "Buy 17 mm",
            Self::Buy42 => "Buy 42 mm",
            Self::Settings => "Settings",
            Self::Map => "Map",
            Self::Debug => "Debug panel",
            Self::Help => "Hold help",
            Self::Roster => "Hold team roster",
            Self::Colliders => "Physics colliders",
            Self::Pause => "Pause match",
            Self::Step => "Step match",
            Self::Start => "Start / reset match",
            Self::Rune => "Activate rune",
        }
    }
    /// Bits describe modes in which an action is usable: pilot, spectator, referee.
    fn contexts(self) -> u8 {
        match self {
            Self::Spin
            | Self::Camera
            | Self::Buy17
            | Self::Buy42
            | Self::AutoAim
            | Self::AutoFire
            | Self::AimMode => 1,
            Self::Up | Self::Down => 6,
            Self::Pause | Self::Step | Self::Start | Self::Rune => 7, // local pilots referee too
            _ => 7,
        }
    }
    fn default_binding(self) -> Binding {
        use KeyCode::*;
        Binding::Key(match self {
            Self::Forward => KeyW,
            Self::Backward => KeyS,
            Self::Left => KeyA,
            Self::Right => KeyD,
            Self::Up => Space,
            Self::Down => ShiftLeft,
            Self::Fast => ControlLeft,
            Self::Spin => KeyR,
            Self::Camera => KeyV,
            Self::Fire => return Binding::Mouse(MouseButton::Left),
            Self::AutoAim | Self::AutoFire => return Binding::Mouse(MouseButton::Right),
            Self::AimMode => KeyG,
            Self::Buy17 => KeyO,
            Self::Buy42 => KeyI,
            Self::Settings => KeyP,
            Self::Map => KeyM,
            Self::Debug => F3,
            Self::Help => F12,
            Self::Roster => Tab,
            Self::Colliders => KeyC,
            Self::Pause => F6,
            Self::Step => F7,
            Self::Start => F5,
            Self::Rune => KeyF,
        })
    }
}
/// One physical input a player can assign to an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Binding {
    /// A keyboard key.
    Key(KeyCode),
    /// A mouse button, side buttons included.
    Mouse(MouseButton),
}
impl Binding {
    /// The binding as menus show it: a `Debug` key name with a leading `Key`
    /// trimmed, or `Mouse <button>`.
    pub fn label(self) -> String {
        match self {
            Self::Key(key) => format!("{key:?}").trim_start_matches("Key").to_owned(),
            Self::Mouse(button) => format!("Mouse {button:?}"),
        }
    }
    /// True for an input the app keeps for itself. Escape closes panels, so it
    /// is never stored as a binding.
    pub fn reserved(self) -> bool {
        matches!(self, Self::Key(KeyCode::Escape))
    }
    fn active(
        self,
        keys: &ButtonInput<KeyCode>,
        buttons: Option<&ButtonInput<MouseButton>>,
        edge: bool,
    ) -> bool {
        match self {
            Self::Key(key) => {
                if edge {
                    keys.just_pressed(key)
                } else {
                    keys.pressed(key)
                }
            }
            Self::Mouse(button) => buttons.is_some_and(|b| {
                if edge {
                    b.just_pressed(button)
                } else {
                    b.pressed(button)
                }
            }),
        }
    }
}
/// The player's bindings and look options, saved with the other preferences.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlsSettings {
    /// Up to two bindings per action, primary then alternate. An action absent
    /// from the map keeps its default primary binding.
    pub bindings: BTreeMap<InputAction, [Option<Binding>; 2]>,
    /// Negate vertical look sensitivity, so mouse motion down raises the gun.
    pub invert_y: bool,
    /// Choose the auto-aim target class by hand with [`InputAction::AimMode`]
    /// instead of letting auto aim pick any target; off by default.
    pub manual_aim_mode: bool,
}
impl ControlsSettings {
    /// The two binding slots for `action`: the stored ones when the map has the
    /// action, otherwise its default primary binding and an empty slot.
    pub fn slots(&self, action: InputAction) -> [Option<Binding>; 2] {
        self.bindings
            .get(&action)
            .copied()
            .unwrap_or([Some(action.default_binding()), None])
    }
    /// Both slots of `action` as one menu string, or `Unbound` when it has none.
    pub fn label(&self, action: InputAction) -> String {
        let labels: Vec<_> = self
            .slots(action)
            .into_iter()
            .flatten()
            .map(Binding::label)
            .collect();
        if labels.is_empty() {
            "Unbound".into()
        } else {
            labels.join(" / ")
        }
    }
    /// True while any slot of `action` is held down.
    pub fn pressed(
        &self,
        action: InputAction,
        keys: &ButtonInput<KeyCode>,
        buttons: Option<&ButtonInput<MouseButton>>,
    ) -> bool {
        self.active(action, keys, buttons, false)
    }
    /// True on the frame any slot of `action` went down.
    pub fn just_pressed(
        &self,
        action: InputAction,
        keys: &ButtonInput<KeyCode>,
        buttons: Option<&ButtonInput<MouseButton>>,
    ) -> bool {
        self.active(action, keys, buttons, true)
    }
    fn active(
        &self,
        action: InputAction,
        keys: &ButtonInput<KeyCode>,
        buttons: Option<&ButtonInput<MouseButton>>,
        edge: bool,
    ) -> bool {
        self.slots(action)
            .into_iter()
            .flatten()
            .any(|binding| binding.active(keys, buttons, edge))
    }
    /// Every `(action, slot)` already holding `binding` in a mode that overlaps
    /// `action`'s, including `action`'s other slot. Auto aim and auto fire are
    /// exempt so one button can hold both; the caller decides what to clear.
    pub fn conflicts(
        &self,
        action: InputAction,
        slot: usize,
        binding: Binding,
    ) -> Vec<(InputAction, usize)> {
        InputAction::ALL
            .into_iter()
            .filter(|other| action.contexts() & other.contexts() != 0)
            .filter(|other| {
                !matches!(
                    (action, *other),
                    (InputAction::AutoAim, InputAction::AutoFire)
                        | (InputAction::AutoFire, InputAction::AutoAim)
                )
            })
            .flat_map(|other| {
                self.slots(other)
                    .into_iter()
                    .enumerate()
                    .filter_map(move |(index, value)| {
                        (value == Some(binding) && (other, index) != (action, slot))
                            .then_some((other, index))
                    })
            })
            .collect()
    }
    /// Store `value` in slot `slot` of `action`, or clear that slot with `None`,
    /// and write the pair back into the map. Panics if `slot` is not 0 or 1.
    pub fn set(&mut self, action: InputAction, slot: usize, value: Option<Binding>) {
        let mut slots = self.slots(action);
        slots[slot] = value;
        self.bindings.insert(action, slots);
    }
    /// The look sensitivity to use: `sensitivity`, negated when `invert_y` is set.
    pub fn vertical_sensitivity(&self, sensitivity: f32) -> f32 {
        if self.invert_y {
            -sensitivity
        } else {
            sensitivity
        }
    }
    /// Clear every reserved binding. Loading a profile runs this, so an older or
    /// hand-edited file cannot give Escape to an action.
    pub fn validate(&mut self) {
        for slots in self.bindings.values_mut() {
            for value in slots.iter_mut() {
                if value.is_some_and(Binding::reserved) {
                    *value = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_aim_and_fire_share_a_binding_but_remain_independent() {
        let mut controls = ControlsSettings::default();
        let right = Binding::Mouse(MouseButton::Right);
        assert!(
            controls
                .conflicts(InputAction::AutoAim, 0, right)
                .is_empty()
        );
        assert!(
            controls
                .conflicts(InputAction::AutoFire, 0, right)
                .is_empty()
        );
        assert!(
            !controls
                .conflicts(InputAction::Forward, 0, right)
                .is_empty()
        );
        controls.set(InputAction::AutoFire, 0, None);
        let restored: ControlsSettings =
            serde_json::from_str(&serde_json::to_string(&controls).unwrap()).unwrap();
        assert_eq!(restored.slots(InputAction::AutoAim)[0], Some(right));
        assert_eq!(restored.slots(InputAction::AutoFire)[0], None);
    }

    #[test]
    fn alternate_mouse_binding_and_release_use_same_named_action() {
        let mut controls = ControlsSettings::default();
        controls.set(
            InputAction::Forward,
            1,
            Some(Binding::Mouse(MouseButton::Other(4))),
        );
        let mut buttons = ButtonInput::default();
        let keys = ButtonInput::default();
        buttons.press(MouseButton::Other(4));
        assert!(controls.just_pressed(InputAction::Forward, &keys, Some(&buttons)));
        buttons.clear();
        assert!(controls.pressed(InputAction::Forward, &keys, Some(&buttons)));
        assert!(!controls.just_pressed(InputAction::Forward, &keys, Some(&buttons)));
        buttons.release(MouseButton::Other(4));
        assert!(!controls.pressed(InputAction::Forward, &keys, Some(&buttons)));
    }
    #[test]
    fn conflicts_respect_exclusive_modes_but_local_referee_overlaps_pilot() {
        let controls = ControlsSettings::default();
        assert!(
            controls
                .conflicts(InputAction::Up, 0, Binding::Key(KeyCode::KeyR))
                .is_empty()
        );
        assert_eq!(
            controls.conflicts(InputAction::Pause, 0, Binding::Key(KeyCode::KeyR)),
            vec![(InputAction::Spin, 0)]
        );
        assert_eq!(
            controls.conflicts(InputAction::Forward, 1, Binding::Key(KeyCode::KeyW)),
            vec![(InputAction::Forward, 0)]
        );
    }
}
