//! Font system: cosmic-text for shaping/metrics, bridged to krilla fonts for embedding.

use std::collections::HashMap;

use cosmic_text::fontdb;
use krilla::text::Font as KrillaFont;

pub struct Fonts {
    pub system: cosmic_text::FontSystem,
    cache: HashMap<fontdb::ID, Option<KrillaFont>>,
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new()
    }
}

impl Fonts {
    pub fn new() -> Self {
        Self {
            system: cosmic_text::FontSystem::new(),
            cache: HashMap::new(),
        }
    }

    /// A krilla font (embeddable, subsettable) for a cosmic-text/fontdb font id.
    pub fn krilla_font(&mut self, id: fontdb::ID) -> Option<KrillaFont> {
        if let Some(cached) = self.cache.get(&id) {
            return cached.clone();
        }
        let built = self.system.db().with_face_data(id, |data, index| {
            KrillaFont::new(data.to_vec().into(), index)
        });
        let font = built.flatten();
        self.cache.insert(id, font.clone());
        font
    }
}
