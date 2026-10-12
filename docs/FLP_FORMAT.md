# Formato FLP (FL Studio) — note di implementazione `mfc-flp`

> La mappatura descritta qui è **derivata esclusivamente da documentazione pubblica della
> community**: sorgenti e documentazione di [PyFLP](https://github.com/demberto/PyFLP)
> (`pyflp/_events.py`, `channel.py`, `pattern.py`, `arrangement.py`, `mixer.py`, `project.py`,
> `plugin.py`), [Monad.FLParser](https://github.com/monadgroup/FLParser) e note "FLP format"
> della community. Nessun codice Image-Line (né decompilato) è stato consultato o usato.

## Struttura del file

Little-endian, due chunk:

| Offset | Campo | Note |
|---|---|---|
| 0 | `"FLhd"` | magic |
| 4 | `u32` = 6 | lunghezza header |
| 8 | `i16` formato | 0 = progetto |
| 10 | `u16` canali | numero di canali del Channel Rack |
| 12 | `u16` PPQ | tick per semiminima (96 nei progetti nuovi) |
| 14 | `"FLdt"` | magic |
| 18 | `u32` | lunghezza del flusso di eventi |
| 22 | eventi | fino a fine chunk |

Ogni evento è `id: u8` seguito da un valore la cui dimensione dipende dall'id:

| Id | Classe | Valore |
|---|---|---|
| 0–63 | BYTE | 1 byte |
| 64–127 | WORD | 2 byte |
| 128–191 | DWORD | 4 byte |
| 192–255 | TEXT/DATA | lunghezza varint (7 bit per byte, LSB first, bit 7 = continua) + payload |

Testo: id 192–207 più 231, 239, 241. Da FL 11.5 il testo è UTF-16LE con NUL finale, prima
ASCII/ANSI; la versione (evento 199) è sempre ASCII e decide la codifica (regola PyFLP:
`[major, minor] >= [11, 5]`).

## Livello eventi (lossless)

`parse` / `write` fanno round-trip **byte-identico** di qualsiasi input ben formato:
varint non minimi sono conservati (`RawEvent::varint_len`), byte dopo il chunk `FLdt`
sono conservati (`FlpFile::trailing`). Input troncati o corrotti producono `FlpError`
(mai panic; nessuna allocazione prima di verificare la lunghezza disponibile).
`inspect` produce un riepilogo per evento (id, nome, classe, dimensione, anteprima).

## Eventi mappati nel modello

| Area | Id (nome) | Mappatura |
|---|---|---|
| Progetto | 199 FLVersion | `info.fl_version`, codifica testo |
| | 194 Title, 195 Comments (198 RTF come ripiego), 197 Url, 206 Genre, 207 Artists | `info.*` |
| | 156 Tempo (bpm×1000); 66+93 tempo coarse/fine (vecchi) | `tempo` |
| | header PPQ | `time.ppq` (tick non riscalati) |
| | 17/18 TimeSigNum/Beat | `time.signature` |
| | 12 MainVolume (%), 80 MainPitch (cent), 11 Swing (0–128) | `master_volume`, `master_pitch`, `swing` |
| | 67 CurrentPattern | `current_pattern` |
| Canali | 64 ChanNew (iid) | nuovo canale, in ordine |
| | 21 ChanType: 0 sampler, 2 nativo/audio clip, 3 layer, 4 strumento, 5 automazione | `ChannelKind` |
| | 201 PluginInternalName, 203 PluginName, 192 nome vecchio | plugin e nome |
| | 213 PluginData | `opaque_state` |
| | 196 SamplePath | `plugin.sample_path` |
| | 219 Levels (pan 0–12800, vol 0–12800, pitch cent); 72/73, 2/3 vecchi | `volume = raw/12800`, `pan = (raw-6400)/6400`, `pitch_cents` |
| | 22 RoutedTo (i8) | `mixer_insert` |
| | 128 PluginColor (RGBA), 0 Enabled, 135 RootNote, 94 Children | `color`, `muted`, `root_note`, `layer_children` |
| Pattern | 65 PatNew (due blocchi: note, poi metadati), 193 nome, 150 colore | `Pattern` |
| | 224 Notes, record 24 byte: pos u32, flags u16 (slide = bit 3), canale iid u16, len u32, key u16, group u16, fine pitch u8 (120 = 0), u8, release u8, midi ch u8, pan u8 (64 centro), velocity u8 (0–128), mod X/Y u8 | `Note` (len 0 = step → 1 step) |
| Playlist | 99 ArrangementNew, 100 ArrangementCurrent | si importa l'arrangement corrente |
| | 233 PlaylistItems, record 32 byte (60 da FL 21): pos u32, pattern_base u16 (20480), item_index u16, len u32, track_rvidx u16 (499 − traccia), group u16, 2 byte, flags u16 (muted = 0x2000), 4 byte, start/end offset f32 | `PlaylistClip` (item_index > base → pattern, altrimenti canale audio) |
| | 238 TrackData (iid, colore, icona, enabled…), 239 TrackName | `playlist.tracks` |
| Mixer | 236 InsertFlags (u32 a offset 4: bit0 polarità, bit1 swap, bit2 effetti, bit3 abilitato, bit12 solo) | `muted/solo/phase_invert/swap_lr` |
| | 204 nome, 149 colore, 235 Routing (bool per insert di destinazione) | `name`, `color`, `routes` |
| | 201/213 + 98 SlotIndex (l'indice segue gli eventi plugin) | `effects` |
| | 225 MixerParams, record 12 byte: u32, id u8, u8, `insert<<6 | slot` u16, msg i32. Id 0 slot on, 1 slot mix (−6400..6400), 64–191 livello mandata, 192 volume (0–16000, 12800 = 0 dB), 193 pan (±6400), 194 separazione (±64) | `volume = raw/16000`, `pan`, `stereo_separation`, `Route.gain`, `EffectSlot.enabled/mix` |

Gli insert sono delimitati da 147 InsertOutput (con 235/236 ripetuti come ulteriore confine);
l'indice 0 nel file è il master, l'ultimo (126) è l'insert "current" ed è ignorato.

Plugin: tabella in `crates/mfc-flp/src/plugin_map.rs` (es. "3x Osc" → `mfc.3xosc`,
"Fruity Reeverb 2" → `mfc.reverb`, "Fruity Squeeze" → `mfc.bitcrusher`). Generatori sconosciuti
diventano `mfc.silent` con `foreign_name` + `opaque_state`; effetti sconosciuti diventano
`kind = "foreign"`, slot bypassato, stato preservato.

## Preservato solo a livello eventi

Tutto ciò che non è nella tabella (envelope/LFO, arp, delay e parametri del canale, filtri,
controller dei pattern, time marker, automazioni, wrapper VST 212, EQ dell'insert, timestamp,
display group, ecc.) resta nel `FlpFile` e viene segnalato con un avviso aggregato
in `ImportResult::warnings`; l'import non fallisce mai per contenuti non mappati.

## Export

`from_project` scrive un file "FL 20.8.4.2576" (testo UTF-16) nell'ordine: progetto → note dei
pattern → metadati dei pattern → canali → arrangement (500 tracce) → `MixerParams` → 127 insert
(flag, 10 slot, colore, nome, routing, input/output). Generatori senza equivalente FL e canali
di automazione vengono scritti come sampler vuoti con lo stesso nome.

## Limiti noti / incertezze

- Mappatura insert master/“current”: PyFLP e Monad.FLParser divergono; si segue Monad (master = 0).
- Muted dei clip (0x2000) e unità degli offset dei clip (tick per pattern, beat per audio) da
  Monad.FLParser; PyFLP li lascia non decodificati.
- Range di mix degli slot e dei livelli di mandata non verificati su file reali.
- `TrackData` esportato con i 48 byte documentati (FL 20.9 ne scrive 66).
- Clip di automazione, controller dei pattern, più arrangement, lunghezza dei pattern e note
  oltre MIDI 127 non sono importati (avviso). Lo stato `opaque_state` dei plugin nativi mappati
  viene riscritto tale e quale: modifiche ai parametri mfc non si riflettono in FL.
- L'export non è stato verificato aprendo i file in FL Studio.
