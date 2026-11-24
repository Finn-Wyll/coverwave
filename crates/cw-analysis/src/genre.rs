//! Genre families: free-form genre strings from tags or services, folded
//! into a small set of families, each with the animations it favours.
//!
//! The family *biases* layer selection; the cover still decides most of it
//! (see `mapping`). Pure and deterministic.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GenreFamily {
    Electronic,
    HipHop,
    Rock,
    Metal,
    Punk,
    Pop,
    RnbSoul,
    Jazz,
    Blues,
    Classical,
    Ambient,
    Folk,
    Country,
    Latin,
    Reggae,
    Worship,
    Soundtrack,
    #[default]
    Unknown,
}

/// Keywords per family, checked in this order: more specific families come
/// first, so "pop punk" is Punk and "indie pop" is Pop.
const KEYWORDS: &[(GenreFamily, &[&str])] = &[
    (
        GenreFamily::Metal,
        &["metal", "djent", "grindcore", "deathcore"],
    ),
    (GenreFamily::Punk, &["punk", "emo", "hardcore", "screamo"]),
    (
        GenreFamily::HipHop,
        &[
            "hip hop", "hip-hop", "hiphop", "rap", "trap", "drill", "grime", "boom bap",
        ],
    ),
    (
        GenreFamily::Worship,
        &["worship", "gospel", "christian", "ccm", "praise"],
    ),
    (
        GenreFamily::Electronic,
        &[
            "electronic",
            "electronica",
            "edm",
            "house",
            "techno",
            "trance",
            "dubstep",
            "drum and bass",
            "drum & bass",
            "dnb",
            "synth",
            "electro",
            "idm",
            "garage",
            "dance",
            "breakbeat",
            "hyperpop",
            "phonk",
        ],
    ),
    (
        GenreFamily::RnbSoul,
        &[
            "r&b", "rnb", "r and b", "soul", "funk", "motown", "neo soul",
        ],
    ),
    (GenreFamily::Jazz, &["jazz", "swing", "bebop", "big band"]),
    (GenreFamily::Blues, &["blues"]),
    (
        GenreFamily::Classical,
        &[
            "classical",
            "orchestra",
            "baroque",
            "opera",
            "symphon",
            "romantic era",
            "chamber",
        ],
    ),
    (
        GenreFamily::Ambient,
        &[
            "ambient",
            "new age",
            "drone",
            "chill",
            "lo-fi",
            "lofi",
            "downtempo",
            "meditation",
        ],
    ),
    (
        GenreFamily::Soundtrack,
        &["soundtrack", "score", "film", "video game", "musical"],
    ),
    (GenreFamily::Country, &["country", "bluegrass", "americana"]),
    (
        GenreFamily::Folk,
        &["folk", "acoustic", "singer-songwriter", "singer songwriter"],
    ),
    (
        GenreFamily::Latin,
        &[
            "latin",
            "reggaeton",
            "salsa",
            "bachata",
            "samba",
            "bossa",
            "cumbia",
            "merengue",
            "flamenco",
        ],
    ),
    (GenreFamily::Reggae, &["reggae", "dancehall", "ska", "dub"]),
    (GenreFamily::Pop, &["pop", "k-pop", "kpop", "j-pop"]),
    (
        GenreFamily::Rock,
        &[
            "rock",
            "grunge",
            "indie",
            "alternative",
            "shoegaze",
            "britpop",
        ],
    ),
];

impl GenreFamily {
    /// The family of one genre string ("Indie Pop", "melodic metalcore").
    pub fn of(genre: &str) -> GenreFamily {
        let g = genre.to_lowercase();
        KEYWORDS
            .iter()
            .find(|(_, words)| words.iter().any(|w| g.contains(w)))
            .map_or(GenreFamily::Unknown, |(f, _)| *f)
    }

    /// The family of the first recognisable genre in a list, or of a single
    /// string holding several (`"Rock; Pop"`, `"rap, trap"`).
    pub fn of_any(genres: &str) -> GenreFamily {
        genres
            .split([';', ',', '/', '|'])
            .map(GenreFamily::of)
            .find(|f| *f != GenreFamily::Unknown)
            .unwrap_or(GenreFamily::Unknown)
    }

    pub fn name(self) -> &'static str {
        match self {
            GenreFamily::Electronic => "electronic",
            GenreFamily::HipHop => "hip_hop",
            GenreFamily::Rock => "rock",
            GenreFamily::Metal => "metal",
            GenreFamily::Punk => "punk",
            GenreFamily::Pop => "pop",
            GenreFamily::RnbSoul => "rnb_soul",
            GenreFamily::Jazz => "jazz",
            GenreFamily::Blues => "blues",
            GenreFamily::Classical => "classical",
            GenreFamily::Ambient => "ambient",
            GenreFamily::Folk => "folk",
            GenreFamily::Country => "country",
            GenreFamily::Latin => "latin",
            GenreFamily::Reggae => "reggae",
            GenreFamily::Worship => "worship",
            GenreFamily::Soundtrack => "soundtrack",
            GenreFamily::Unknown => "unknown",
        }
    }

    /// Layer kinds this family favours, with how strongly (0..1).
    pub fn affinities(self) -> &'static [(&'static str, f32)] {
        match self {
            GenreFamily::Electronic => &[
                ("scanlines", 0.9),
                ("pulse_rings", 0.7),
                ("row_glitch", 0.7),
                ("digital_rain", 0.6),
                ("chroma_shift", 0.6),
                ("eq_bars", 0.6),
                ("neon_flicker", 0.5),
                ("light_sweep", 0.4),
            ],
            GenreFamily::HipHop => &[
                ("pulse_rings", 0.9),
                ("subject_glow", 0.7),
                ("eq_bars", 0.7),
                ("row_glitch", 0.5),
                ("chroma_shift", 0.5),
                ("smoke", 0.5),
            ],
            GenreFamily::Rock => &[
                ("lightning", 0.7),
                ("embers", 0.6),
                ("eq_bars", 0.6),
                ("smoke", 0.5),
                ("row_glitch", 0.4),
                ("light_sweep", 0.4),
            ],
            GenreFamily::Metal => &[
                ("embers", 0.9),
                ("lightning", 0.9),
                ("heat_cycle", 0.6),
                ("smoke", 0.6),
                ("flicker", 0.5),
            ],
            GenreFamily::Punk => &[
                ("row_glitch", 0.7),
                ("lightning", 0.5),
                ("embers", 0.5),
                ("scanlines", 0.5),
                ("eq_bars", 0.5),
            ],
            GenreFamily::Pop => &[
                ("light_sweep", 0.7),
                ("twinkle", 0.7),
                ("pulse_rings", 0.6),
                ("subject_glow", 0.6),
                ("bubbles", 0.5),
                ("eq_bars", 0.5),
            ],
            GenreFamily::RnbSoul => &[
                ("subject_glow", 0.8),
                ("smoke", 0.6),
                ("light_sweep", 0.5),
                ("twinkle", 0.5),
                ("pulse_rings", 0.5),
            ],
            GenreFamily::Jazz => &[
                ("smoke", 0.8),
                ("light_sweep", 0.5),
                ("twinkle", 0.5),
                ("subject_glow", 0.5),
                ("ambient", 0.4),
            ],
            GenreFamily::Blues => &[
                ("smoke", 0.7),
                ("rain", 0.6),
                ("light_sweep", 0.4),
                ("flicker", 0.4),
            ],
            GenreFamily::Classical => &[
                ("light_sweep", 0.7),
                ("sun_rays", 0.6),
                ("twinkle", 0.5),
                ("falling_leaves", 0.5),
                ("ambient", 0.4),
            ],
            GenreFamily::Ambient => &[
                ("aurora", 0.9),
                ("starfield", 0.7),
                ("cloud_drift", 0.6),
                ("fog", 0.6),
                ("twinkle", 0.5),
                ("bubbles", 0.4),
            ],
            GenreFamily::Folk => &[
                ("falling_leaves", 0.8),
                ("sun_rays", 0.7),
                ("fireflies", 0.6),
                ("row_sway", 0.5),
                ("dust", 0.4),
            ],
            GenreFamily::Country => &[
                ("sun_rays", 0.8),
                ("dust", 0.7),
                ("heat_shimmer", 0.5),
                ("falling_leaves", 0.5),
            ],
            GenreFamily::Latin => &[
                ("pulse_rings", 0.7),
                ("sun_rays", 0.6),
                ("light_sweep", 0.5),
                ("falling_leaves", 0.5),
            ],
            GenreFamily::Reggae => &[
                ("sun_rays", 0.6),
                ("smoke", 0.6),
                ("pulse_rings", 0.5),
                ("bubbles", 0.5),
            ],
            GenreFamily::Worship => &[
                ("light_sweep", 0.8),
                ("sun_rays", 0.8),
                ("twinkle", 0.6),
                ("aurora", 0.5),
                ("subject_glow", 0.5),
            ],
            GenreFamily::Soundtrack => &[
                ("starfield", 0.6),
                ("light_sweep", 0.6),
                ("aurora", 0.5),
                ("fog", 0.5),
                ("lightning", 0.3),
            ],
            GenreFamily::Unknown => &[],
        }
    }

    /// Added to the cover's energy (-0.2..0.25): heavy genres move more.
    pub fn energy_bias(self) -> f32 {
        match self {
            GenreFamily::Metal | GenreFamily::Punk => 0.25,
            GenreFamily::Electronic | GenreFamily::HipHop | GenreFamily::Rock => 0.15,
            GenreFamily::Pop | GenreFamily::Latin => 0.1,
            GenreFamily::Ambient | GenreFamily::Classical => -0.2,
            GenreFamily::Jazz | GenreFamily::Folk | GenreFamily::Worship => -0.05,
            _ => 0.0,
        }
    }

    /// The rhythm the beat clock plays for this family.
    pub fn beat_pattern(self) -> cw_core::BeatPattern {
        use cw_core::BeatPattern::*;
        match self {
            GenreFamily::Electronic => FourOnFloor,
            GenreFamily::HipHop | GenreFamily::RnbSoul => HalfTime,
            GenreFamily::Reggae => OneDrop,
            GenreFamily::Ambient | GenreFamily::Classical | GenreFamily::Soundtrack => Swell,
            _ => BackBeat,
        }
    }

    /// A plausible tempo when the real one is unknown (beat clock).
    pub fn typical_bpm(self) -> f32 {
        match self {
            GenreFamily::Electronic => 124.0,
            GenreFamily::HipHop => 90.0,
            GenreFamily::Rock | GenreFamily::Pop => 118.0,
            GenreFamily::Metal | GenreFamily::Punk => 150.0,
            GenreFamily::RnbSoul => 95.0,
            GenreFamily::Jazz | GenreFamily::Blues => 100.0,
            GenreFamily::Classical | GenreFamily::Ambient | GenreFamily::Soundtrack => 72.0,
            GenreFamily::Folk | GenreFamily::Country | GenreFamily::Worship => 100.0,
            GenreFamily::Latin => 105.0,
            GenreFamily::Reggae => 80.0,
            GenreFamily::Unknown => 110.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_from_common_tags() {
        let cases = [
            ("Alternative Metal", GenreFamily::Metal),
            ("Nu Metal", GenreFamily::Metal),
            ("Pop Punk", GenreFamily::Punk),
            ("Indie Pop", GenreFamily::Pop),
            ("Indie Rock", GenreFamily::Rock),
            ("Hip-Hop/Rap", GenreFamily::HipHop),
            ("Contemporary Christian", GenreFamily::Worship),
            ("CCM", GenreFamily::Worship),
            ("Deep House", GenreFamily::Electronic),
            ("Neo Soul", GenreFamily::RnbSoul),
            ("Singer-Songwriter", GenreFamily::Folk),
            ("Lo-Fi Beats", GenreFamily::Ambient),
            ("Reggaeton", GenreFamily::Latin),
            ("Soundtrack", GenreFamily::Soundtrack),
            ("", GenreFamily::Unknown),
            ("Spoken Word", GenreFamily::Unknown),
        ];
        for (g, f) in cases {
            assert_eq!(GenreFamily::of(g), f, "{g}");
        }
    }

    #[test]
    fn lists_take_the_first_known() {
        assert_eq!(
            GenreFamily::of_any("Spoken Word; Jazz; Rock"),
            GenreFamily::Jazz
        );
        assert_eq!(
            GenreFamily::of_any("conscious hip hop, rap"),
            GenreFamily::HipHop
        );
        assert_eq!(GenreFamily::of_any("misc"), GenreFamily::Unknown);
    }

    #[test]
    fn every_family_but_unknown_has_affinities() {
        for (f, _) in KEYWORDS {
            assert!(!f.affinities().is_empty(), "{f:?}");
            assert!(f.affinities().iter().all(|(_, w)| (0.0..=1.0).contains(w)));
        }
    }
}
