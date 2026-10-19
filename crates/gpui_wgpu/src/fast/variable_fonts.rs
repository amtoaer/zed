use crate::cosmic_text_system::{CosmicTextSystemState, LoadedFont, check_is_known_emoji_font};
use anyhow::{Context as _, Result};
use collections::HashMap;
use cosmic_text::{Font as CosmicTextFont, FontFeatures as CosmicFontFeatures, Weight};
use gpui::{FontId, FontWeight};
use smallvec::SmallVec;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct VariableFonts(HashMap<(FontId, u16), FontId>);

pub(crate) fn initial_weight(font: &CosmicTextFont, weight: Weight) -> Option<u16> {
    font.as_swash()
        .variations()
        .find_by_tag(swash::Tag::from_be_bytes(*b"wght"))
        .map(|_| weight.0)
}

pub(crate) fn weight(font: &LoadedFont, default: Weight) -> Weight {
    font.fast_weight.map_or(default, Weight)
}

pub(crate) fn coords(font: &LoadedFont) -> SmallVec<[swash::NormalizedCoord; 4]> {
    let Some(weight) = font.fast_weight else {
        return SmallVec::new();
    };
    font.font
        .as_swash()
        .variations()
        .normalized_coords([(swash::Tag::from_be_bytes(*b"wght"), weight as f32)])
        .collect()
}

pub(crate) fn metrics(font: &LoadedFont) -> swash::Metrics {
    font.font.as_swash().metrics(&coords(font))
}

pub(crate) fn needs_instance(
    font: &LoadedFont,
    id: cosmic_text::fontdb::ID,
    weight: Weight,
) -> bool {
    font.font.id() != id || font.fast_weight.is_some_and(|current| current != weight.0)
}

pub(crate) fn resolve(
    state: &mut CosmicTextSystemState,
    id: FontId,
    weight: FontWeight,
) -> Result<FontId> {
    let weight = weight.0.round() as u16;
    if state.loaded_fonts[id.0]
        .fast_weight
        .is_none_or(|current| current == weight)
    {
        return Ok(id);
    }
    if let Some(instance) = state.fast_variable_fonts.0.get(&(id, weight)) {
        return Ok(*instance);
    }
    let loaded = &state.loaded_fonts[id.0];
    let font = state
        .font_system
        .get_font(loaded.font.id(), Weight(weight))
        .context("failed to load variable font instance")?;
    let instance = LoadedFont {
        font,
        features: loaded.features.clone(),
        is_known_emoji_font: loaded.is_known_emoji_font,
        user_fallback_chain: Arc::clone(&loaded.user_fallback_chain),
        fast_weight: Some(weight),
    };
    let instance_id = FontId(state.loaded_fonts.len());
    state.loaded_fonts.push(instance);
    state
        .fast_variable_fonts
        .0
        .insert((id, weight), instance_id);
    Ok(instance_id)
}

pub(crate) fn fallback(
    state: &mut CosmicTextSystemState,
    id: cosmic_text::fontdb::ID,
    weight: cosmic_text::Weight,
) -> Result<FontId> {
    if let Some(ix) = state.loaded_fonts.iter().position(|loaded_font| {
        loaded_font.font.id() == id
            && loaded_font
                .fast_weight
                .is_none_or(|fast_weight| fast_weight == weight.0)
    }) {
        Ok(FontId(ix))
    } else {
        let font = state
            .font_system
            .get_font(id, weight)
            .context("failed to get fallback font from cosmic-text font system")?;
        let face = state
            .font_system
            .db()
            .face(id)
            .context("fallback font face not found in cosmic-text database")?;

        let fast_weight = initial_weight(&font, weight);
        let font_id = FontId(state.loaded_fonts.len());
        state.loaded_fonts.push(LoadedFont {
            font,
            features: CosmicFontFeatures::new(),
            is_known_emoji_font: check_is_known_emoji_font(&face.post_script_name),
            user_fallback_chain: Arc::from(Vec::new()),
            fast_weight,
        });

        Ok(font_id)
    }
}
