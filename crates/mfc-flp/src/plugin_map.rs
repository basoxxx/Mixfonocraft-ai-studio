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

/// Formato di un plugin esterno.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalFormat {
    Vst3,
    Clap,
}

impl ExternalFormat {
    pub fn prefix(self) -> &'static str {
        match self {
            ExternalFormat::Vst3 => "vst3:",
            ExternalFormat::Clap => "clap:",
        }
    }
    /// Formato dedotto dall'estensione del percorso (`.vst3` / `.clap`, senza distinzione
    /// di maiuscole); `None` per VST2 (`.dll`, `.so`), AU e percorsi ignoti.
    pub fn from_path(path: &str) -> Option<Self> {
        let p = path.trim_end_matches(['/', '\\']).to_ascii_lowercase();
        if p.ends_with(".vst3") {
            Some(ExternalFormat::Vst3)
        } else if p.ends_with(".clap") {
            Some(ExternalFormat::Clap)
        } else {
            None
        }
    }
}

/// Plugin esterno scomposto dal kind `vst3:<percorso>::<id>` / `clap:<percorso>::<id>`
/// (stessa sintassi di `mfc-host`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalPlugin {
    pub format: ExternalFormat,
    pub path: String,
    pub id: String,
}

impl ExternalPlugin {
    pub fn parse(kind: &str) -> Option<Self> {
        let (format, rest) = match kind.strip_prefix("vst3:") {
            Some(r) => (ExternalFormat::Vst3, r),
            None => (ExternalFormat::Clap, kind.strip_prefix("clap:")?),
        };
        let (path, id) = rest.rsplit_once("::")?;
        if path.is_empty() || id.is_empty() {
            return None;
        }
        Some(Self {
            format,
            path: path.to_string(),
            id: id.to_string(),
        })
    }

    pub fn kind(&self) -> String {
        format!("{}{}::{}", self.format.prefix(), self.path, self.id)
    }

    /// Nome di ripiego: nome del file senza estensione.
    pub fn file_stem(&self) -> String {
        let base = self.path.trim_end_matches(['/', '\\']);
        let file = base.rsplit(['/', '\\']).next().unwrap_or(base);
        match file.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => stem.to_string(),
            _ => file.to_string(),
        }
    }

    /// Class id VST3 (32 cifre esadecimali, ordine dei byte del TUID) → 16 byte.
    fn guid_bytes(&self) -> Option<Vec<u8>> {
        let s = self.id.trim();
        if s.len() != 32 || !s.is_ascii() {
            return None;
        }
        (0..16)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
            .collect()
    }
}

/// Dati del "Fruity Wrapper" per un plugin esterno: nome, produttore, percorso, id e stato.
///
/// Il record `GUID` contiene i 16 byte del class id per i VST3; per i CLAP (per cui la
/// struttura usata da FL 2024 non è documentata) contiene l'id del plugin in UTF-8.
pub fn wrapper_data(ext: &ExternalPlugin, name: &str, vendor: &str, state: Option<&[u8]>) -> Vec<u8> {
    use crate::structs::{vst_record as r, FlWrapperData, WRAPPER_DATA_TYPE};
    let guid = match ext.format {
        ExternalFormat::Vst3 => ext.guid_bytes().unwrap_or_else(|| ext.id.as_bytes().to_vec()),
        ExternalFormat::Clap => ext.id.as_bytes().to_vec(),
    };
    let mut records = vec![
        (r::NAME, name.as_bytes().to_vec()),
        (r::VENDOR, vendor.as_bytes().to_vec()),
        (r::PLUGIN_PATH, ext.path.as_bytes().to_vec()),
        (r::GUID, guid),
    ];
    if let Some(s) = state {
        records.push((r::STATE, s.to_vec()));
    }
    FlWrapperData {
        kind: WRAPPER_DATA_TYPE,
        records,
    }
    .write()
}

/// Plugin esterno riconosciuto nei dati di un "Fruity Wrapper".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedExternal {
    pub plugin: ExternalPlugin,
    /// Nome del plugin (record `Name`), se presente.
    pub name: Option<String>,
    /// Blob di stato (record `State`).
    pub state: Option<Vec<u8>>,
}

/// Riconosce un VST3/CLAP nei dati del wrapper: servono il percorso (con estensione
/// `.vst3`/`.clap`) e un id (record `GUID`). Altrimenti `None` (VST2, dati non validi…).
pub fn external_from_wrapper(data: &[u8]) -> Option<WrappedExternal> {
    use crate::structs::{vst_record as r, FlWrapperData};
    let w = FlWrapperData::parse(data)?;
    let path = w.text(r::PLUGIN_PATH).filter(|p| !p.is_empty())?;
    let format = ExternalFormat::from_path(&path)?;
    let guid = w.record(r::GUID)?;
    let id = match format {
        ExternalFormat::Vst3 if guid.len() == 16 => guid.iter().map(|b| format!("{b:02X}")).collect(),
        _ => {
            let s = std::str::from_utf8(guid).ok()?.trim_end_matches('\0').trim();
            if s.is_empty() || s.contains("::") {
                return None;
            }
            s.to_string()
        }
    };
    Some(WrappedExternal {
        plugin: ExternalPlugin { format, path, id },
        name: w.text(r::NAME).filter(|n| !n.is_empty()),
        state: w.record(r::STATE).map(<[u8]>::to_vec),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_wrapper_roundtrip() {
        let v = ExternalPlugin::parse("vst3:C:\\VST3\\Big Synth.vst3::00112233445566778899AABBCCDDEEFF").unwrap();
        assert_eq!(v.format, ExternalFormat::Vst3);
        assert_eq!(v.file_stem(), "Big Synth");
        let d = wrapper_data(&v, "Big Synth", "", Some(&[7, 8, 9]));
        let w = external_from_wrapper(&d).unwrap();
        assert_eq!(w.plugin, v);
        assert_eq!(w.plugin.kind(), v.kind());
        assert_eq!(w.name.as_deref(), Some("Big Synth"));
        assert_eq!(w.state.as_deref(), Some(&[7u8, 8, 9][..]));

        let c = ExternalPlugin::parse("clap:/usr/lib/clap/fx.clap::com.vendor.fx").unwrap();
        assert_eq!(c.file_stem(), "fx");
        let w = external_from_wrapper(&wrapper_data(&c, "FX", "Vendor", None)).unwrap();
        assert_eq!(w.plugin, c);
        assert_eq!(w.state, None);

        // VST2 (.dll) e kind non validi non sono riconosciuti.
        let v2 = ExternalPlugin {
            format: ExternalFormat::Vst3,
            path: "C:\\VST\\old.dll".into(),
            id: "ABCD".into(),
        };
        assert_eq!(external_from_wrapper(&wrapper_data(&v2, "Old", "", None)), None);
        assert_eq!(ExternalPlugin::parse("vst3:/x.vst3"), None);
        assert_eq!(ExternalPlugin::parse("mfc.reverb"), None);
    }

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
