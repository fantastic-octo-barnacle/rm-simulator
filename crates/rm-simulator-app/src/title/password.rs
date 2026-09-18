// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Visual masking for Bevy's plain-text editor, which has no password mode.
//! Keep the editor and its selection intact; substitute only the rendered glyphs
//! after layout and restore them before the next layout. No secret is copied into
//! another text widget. If the mask font is not ready, render no password glyphs.
use bevy::{
    feathers::{controls::FeathersCheckbox, theme::ThemedText},
    prelude::*,
    text::{PositionedGlyph, TextLayoutInfo},
    ui::UiSystems,
    ui_widgets::{ValueChange, checkbox_self_update},
};

#[derive(Component)]
struct Password {
    revealed: bool,
    original: Option<Vec<PositionedGlyph>>,
    mask: Entity,
}
#[derive(Component)]
struct MaskGlyph;

pub(super) struct PasswordPlugin;
impl Plugin for PasswordPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, restore.before(UiSystems::Content))
            .add_systems(PostUpdate, conceal.after(UiSystems::PostLayout));
    }
}

/// Add a separate Show password checkbox; each field starts concealed.
pub(super) fn attach(commands: &mut Commands, parent: Entity, input: Entity) {
    let mask = commands
        .spawn((
            ChildOf(parent),
            MaskGlyph,
            Text::new("•"),
            TextColor(Color::NONE),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(input).insert(Password {
        revealed: false,
        original: None,
        mask,
    });
    commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Show password") ThemedText } }
        on(move |change: On<ValueChange<bool>>, mut passwords: Query<&mut Password>| {
            if let Ok(mut password) = passwords.get_mut(input) { password.revealed = change.value; }
        })
    }).insert(ChildOf(parent)).observe(checkbox_self_update);
}

fn restore(
    mut inputs: Query<(&mut Password, &mut TextLayoutInfo, &TextFont), Without<MaskGlyph>>,
    mut masks: Query<&mut TextFont, With<MaskGlyph>>,
) {
    for (mut password, mut layout, font) in &mut inputs {
        if let Some(original) = password.original.take() {
            layout.glyphs = original;
        }
        if let Ok(mut mask_font) = masks.get_mut(password.mask)
            && *mask_font != *font
        {
            *mask_font = font.clone();
        }
    }
}

fn conceal(
    mut inputs: Query<(&mut Password, &mut TextLayoutInfo), Without<MaskGlyph>>,
    masks: Query<&TextLayoutInfo, With<MaskGlyph>>,
) {
    for (mut password, mut layout) in &mut inputs {
        if password.revealed {
            continue;
        }
        password.original = Some(layout.glyphs.clone());
        if let Some(mask) = masks
            .get(password.mask)
            .ok()
            .and_then(|layout| layout.glyphs.first())
        {
            for glyph in &mut layout.glyphs {
                glyph.atlas_info = mask.atlas_info.clone();
                glyph.position.y = mask.position.y;
            }
        } else {
            layout.glyphs.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::text::{EditableText, GlyphAtlasInfo};

    #[test]
    fn masking_preserves_the_editor_and_restores_glyphs_when_revealed() {
        let mut app = App::new();
        app.add_systems(Update, (restore, conceal).chain());
        let glyph = |x| PositionedGlyph {
            position: Vec2::new(x, 8.),
            atlas_info: GlyphAtlasInfo {
                texture: default(),
                rect: Rect::new(0., 0., x, 10.),
                offset: Vec2::ZERO,
                is_alpha_mask: true,
            },
            section_index: 0,
            line_index: 0,
        };
        let mask = app
            .world_mut()
            .spawn((
                MaskGlyph,
                TextFont::default(),
                TextLayoutInfo {
                    glyphs: vec![glyph(4.)],
                    ..default()
                },
            ))
            .id();
        let input = app
            .world_mut()
            .spawn((
                Password {
                    revealed: false,
                    original: None,
                    mask,
                },
                EditableText::new("秘密"),
                TextFont::default(),
                TextLayoutInfo {
                    glyphs: vec![glyph(10.), glyph(20.)],
                    ..default()
                },
            ))
            .id();
        for _ in 0..2 {
            app.update();
            let layout = app.world().get::<TextLayoutInfo>(input).unwrap();
            assert_eq!(layout.glyphs[0].atlas_info.rect.max.x, 4.);
            assert_eq!(layout.glyphs[1].position.x, 20.);
            assert_eq!(
                app.world()
                    .get::<EditableText>(input)
                    .unwrap()
                    .value()
                    .to_string(),
                "秘密"
            );
        }
        app.world_mut().get_mut::<Password>(input).unwrap().revealed = true;
        app.update();
        assert_eq!(
            app.world().get::<TextLayoutInfo>(input).unwrap().glyphs[0]
                .atlas_info
                .rect
                .max
                .x,
            10.
        );
        app.world_mut().get_mut::<Password>(input).unwrap().revealed = false;
        app.world_mut()
            .get_mut::<TextLayoutInfo>(mask)
            .unwrap()
            .glyphs
            .clear();
        app.update();
        assert!(
            app.world()
                .get::<TextLayoutInfo>(input)
                .unwrap()
                .glyphs
                .is_empty()
        );
    }
}
