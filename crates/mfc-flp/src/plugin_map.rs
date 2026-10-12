//! Tabella di corrispondenza tra plugin nativi di FL Studio (nome interno) e id Mixfonocraft.

/// Plugin generatori: (nome interno FL, id mfc). Il primo nome per id è quello usato in export.
const GENERATORS: &[(&str, &str)] = &[
    ("3x Osc", "mfc.3xosc"),
    ("BooBass", "mfc.boobass"),
    ("Fruit Kick", "mfc.kick"),
    ("Fruity Kick", "mfc.kick"),
];

/// Plugin effetto: (nome interno FL, id mfc).
const EFFECTS: &[(&str, &str)] = &[
    ("Fruity Reeverb 2", "mfc.reverb"),
    ("Fruity Reeverb", "mfc.reverb"),
    ("Fruity Delay 3", "mfc.delay"),
    ("Fruity Delay 2", "mfc.delay"),
    ("Fruity Delay", "mfc.delay"),
    ("Fruity parametric EQ 2", "mfc.eq"),
    ("Fruity parametric EQ", "mfc.eq"),
    ("Fruity Compressor", "mfc.compressor"),
    ("Fruity Limiter", "mfc.limiter"),
    ("Fruity Filter", "mfc.filter"),
    ("Fruity Chorus", "mfc.chorus"),
    ("Fruity Flanger", "mfc.flanger"),
    ("Fruity Phaser", "mfc.phaser"),
    ("Fruity Fast Dist", "mfc.distortion"),
    ("Fruity Soft Clipper", "mfc.distortion"),
    ("Fruity Blood Overdrive", "mfc.distortion"),
    ("Fruity WaveShaper", "mfc.distortion"),
    ("Distructor", "mfc.distortion"),
    ("Fruity Squeeze", "mfc.bitcrusher"),
    ("Fruity Stereo Shaper", "mfc.stereo"),
    ("Fruity Stereo Enhancer", "mfc.stereo"),
    ("Fruity Balance", "mfc.gain"),
];

/// Id assegnato ai generatori non supportati (stato preservato in `opaque_state`).
pub const FOREIGN_GENERATOR: &str = "mfc.silent";
/// Id assegnato agli effetti non supportati (slot bypassato, stato preservato).
pub const FOREIGN_EFFECT: &str = "foreign";
/// Nome interno dei plugin VST/AU in FL.
pub const FL_WRAPPER: &str = "Fruity Wrapper";

fn lookup(table: &'static [(&'static str, &'static str)], fl_name: &str) -> Option<&'static str> {
    let n = fl_name.trim();
    table.iter().find(|(f, _)| f.eq_ignore_ascii_case(n)).map(|(_, m)| *m)
}

fn reverse(table: &'static [(&'static str, &'static str)], kind: &str) -> Option<&'static str> {
    table.iter().find(|(_, m)| *m == kind).map(|(f, _)| *f)
}

/// Generatore FL → id mfc.
pub fn generator_kind(fl_name: &str) -> Option<&'static str> {
    lookup(GENERATORS, fl_name)
}

/// Effetto FL → id mfc.
pub fn effect_kind(fl_name: &str) -> Option<&'static str> {
    lookup(EFFECTS, fl_name)
}

/// Id mfc → nome interno del generatore FL equivalente.
pub fn fl_generator_name(kind: &str) -> Option<&'static str> {
    reverse(GENERATORS, kind)
}

/// Id mfc → nome interno dell'effetto FL equivalente.
pub fn fl_effect_name(kind: &str) -> Option<&'static str> {
    reverse(EFFECTS, kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_both_ways() {
        assert_eq!(generator_kind("3x osc"), Some("mfc.3xosc"));
        assert_eq!(effect_kind("Fruity Reeverb 2"), Some("mfc.reverb"));
        assert_eq!(effect_kind("Fruity Squeeze"), Some("mfc.bitcrusher"));
        assert_eq!(effect_kind("Serum"), None);
        assert_eq!(fl_effect_name("mfc.delay"), Some("Fruity Delay 3"));
        assert_eq!(fl_generator_name("mfc.kick"), Some("Fruit Kick"));
        assert_eq!(fl_effect_name("mfc.gate"), None);
    }
}
