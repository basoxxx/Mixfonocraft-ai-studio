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
| | 233 PlaylistItems, record 32 byte (60 da FL 21): pos u32, pattern_base u16 (20480), item_index u16, len u32, track_rvidx u16 (499 − traccia), group u16, 2 byte, flags u16 (muted = 0x2000), 4 byte, start/end offset f32 | `PlaylistClip` (item_index > base → pattern, altrimenti canale audio o di automazione) |
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

## Plugin esterni (VST3 / CLAP) nel "Fruity Wrapper"

FL ospita i plugin di terze parti con il wrapper interno **"Fruity Wrapper"** (201
PluginInternalName). Per un canale il tipo è 4 (`Instrument`, 21 ChanType); per uno slot del
mixer valgono gli stessi eventi plugin seguiti da 98 SlotIndex. Il nome visualizzato va in
203 PluginName. Il contenuto di 213 PluginData segue la struttura documentata da PyFLP
(`VSTPluginEvent`):

| Offset | Campo | Note |
|---|---|---|
| 0 | `u32` tipo | 8 o 10 nei file reali; l'export scrive 10 |
| 4… | record ripetuti | `u32 id`, `u64 lunghezza`, `byte[lunghezza]` fino a fine evento |

Id dei record (PyFLP `_VSTPluginEventID`): 1 MIDI, 2 Flags, 30 IO, 31 Inputs, 32 Outputs,
50 PluginInfo, 51 FourCC (solo VST2), **52 GUID**, **53 State**, **54 Name**, **55 PluginPath**,
**56 Vendor**, 57 sconosciuto. Testi in UTF-8 senza terminatore.

Export (`vst3:<percorso>::<id>` e `clap:<percorso>::<id>`, sintassi di `mfc-host`), record
scritti nell'ordine 54 Name (`foreign_name`, o nome del file senza estensione), 56 Vendor (vuoto:
il modello non lo conserva), 55 PluginPath (percorso del kind), 52 GUID, 53 State
(`opaque_state` tale e quale, se presente). GUID: per i VST3 i 16 byte del class id nell'ordine
delle 32 cifre esadecimali del kind (ordine del TUID); per i CLAP l'id del plugin in UTF-8.

Import: un "Fruity Wrapper" i cui dati si scompongono esattamente in record, con 55 PluginPath
che termina in `.vst3`/`.clap` e un 52 GUID (16 byte per i VST3 → 32 cifre maiuscole; testo
UTF-8 per i CLAP o i VST3 non a 16 byte) diventa `kind = vst3:…`/`clap:…`, `opaque_state` = record
53, `foreign_name` = record 54. Gli effetti esterni importati restano abilitati. In tutti gli
altri casi (VST2 `.dll`/`.so`, AU, dati non strutturati, GUID assente) il comportamento resta
quello dei plugin sconosciuti (`mfc.silent`/`foreign` con tutto il blob in `opaque_state`).

**Best-effort**:
- l'evento 212 PluginWrapper (flag, pagina, dimensioni della finestra) non viene scritto;
- non vengono scritti i record 1/2/30–32/50/57 (MIDI, flag, I/O): FL dovrebbe usare i default;
- l'ordine dei byte del GUID VST3 in FL non è documentato (PyFLP issue #8): si usa l'ordine del
  TUID; se FL non trova la classe il plugin può risultare mancante;
- il formato del blob 53 State usato da FL per i VST3 (componente + controller) e per i CLAP non
  è documentato: si scrive lo stato salvato da Mixfonocraft, che FL potrebbe non saper caricare;
- la struttura dei dati del wrapper per i CLAP (FL 2024+) non è documentata pubblicamente: si
  assume la stessa dei VST con GUID testuale;
- all'import i record diversi da 52–55 vanno persi nel modello (restano nel `FlpFile`).

## Clip di automazione

Ogni `AutomationClip` esportabile diventa un canale di **tipo 5** (`Automation`) aggiunto dopo i
canali normali (iid consecutivi; `n_channels` nell'header li include):

`64 ChanNew` → `21 ChanType = 5` → `203 PluginName` (nome della clip) → `0 ChanEnabled = 1` →
`234 ChanAutomation` → `227 RemoteController`.

234 ChanAutomation (layout PyFLP `AutomationEvent`):

| Offset | Campo | Note |
|---|---|---|
| 0 | `u32` | "sempre 1" secondo PyFLP; export: 1 |
| 4 | `i32` | quantità LFO (−128..128); export: 0 |
| 8 | 9 byte | sconosciuti; export: 0 |
| 17 | `u32` | numero di punti |
| 21 | punti da 24 byte | `f64` scarto in X dal punto precedente, `f64` valore 0..1, `f32` tensione, 4 byte sconosciuti (0) |
| … | coda | sconosciuta (PyFLP: fino a 112 byte); export: nessuna |

La posizione assoluta di un punto è la somma degli scarti; PyFLP la descrive come "PPQ
dependant", quindi è trattata come **tick** del PPQ dell'header. Il valore è normalizzato sul
range del parametro FL; la tensione è scritta come −1..1.

227 RemoteController (20 byte, PyFLP `RemoteControllerEvent`): `u16`+`u8`+`u8` sconosciuti (0),
`u16 parameter_data` (bit 15 = parametro di un plugin VST), `i16 destination_data`, 12 byte
sconosciuti (0). L'evento è scritto **dentro il blocco del canale di automazione** (il controller
sorgente è implicito); la codifica di bersagli e parametri è una convenzione best-effort:

| `AutomationTarget` | destination | parameter | Range reale ↔ 0..1 |
|---|---|---|---|
| `ChannelVolume` | iid del canale | 0 | 0..1 (raw ChanLevels/12800) |
| `ChannelPan` | iid del canale | 1 | −1..1 |
| `ChannelPitch` | iid del canale | 4 | −4800..4800 cent |
| `MixerVolume` | `0x2000 \| insert<<6 \| 0x3F` | 192 (come MixerParams) | 0..1 (raw/16000) |
| `MixerPan` | `0x2000 \| insert<<6 \| 0x3F` | 193 | −1..1 |
| `MasterVolume` | −1 | 0 | 0..1.25 |
| `Tempo` | −1 | 1 | 10..522 BPM |

`ChannelParam`, `EffectParam` ed `EffectMix` non hanno un equivalente documentato: la clip e i
suoi elementi di Playlist vengono saltati con un avviso in `ExportResult::warnings`
(`from_project_report`). I valori fuori dal range FL vengono limitati (avviso).

Playlist: l'elemento di una clip di automazione è un item "canale" (come le audio clip, PyFLP
`ChannelPLItem`): `item_index` = iid del canale di automazione, offset di inizio/fine in beat
(−1 se nullo).

Import: un canale di tipo 5 con 234 decodificabile e 227 (nel suo blocco) riconosciuto diventa
un `AutomationClip` (`min`/`max` = range della tabella, punti con posizione arrotondata al tick,
valore e tensione limitati) e **non** un canale; i suoi item di Playlist diventano
`ClipSource::Automation`. Canali di automazione senza collegamento riconosciuto (tipicamente i
file FL reali, dove il collegamento non segue questa convenzione) restano canali
`ChannelKind::Automation` vuoti con avviso, come prima. Un 227 fuori da un canale di tipo 5
resta non mappato.

**Best-effort**: la codifica di `destination_data`/`parameter_data` non è decodificata da PyFLP
(solo `parameter & 0x7FFF` e il bit VST); gli id dei parametri di canale seguono la numerazione
"REC_Chan_*" nota dalla community, quelli del mixer gli id di MixerParams, quelli globali sono
una convenzione di Mixfonocraft. Anche l'unità delle posizioni (tick), il range della tensione
(PyFLP indica 0..1), l'uso di 219 Levels come Min/Max nei canali di automazione (non scritto) e
la coda dell'evento 234 non sono verificati su file reali. Il round-trip Mixfonocraft → FLP →
Mixfonocraft è garantito dai test; l'apertura in FL Studio no.

## Preservato solo a livello eventi

Tutto ciò che non è nella tabella (envelope/LFO, arp, delay e parametri del canale, filtri,
controller dei pattern, time marker, automazioni FL non riconosciute, wrapper VST 212, record VST
diversi da 52–55, EQ dell'insert, timestamp,
display group, ecc.) resta nel `FlpFile` e viene segnalato con un avviso aggregato
in `ImportResult::warnings`; l'import non fallisce mai per contenuti non mappati.

## Export

`from_project` scrive un file "FL 20.8.4.2576" (testo UTF-16) nell'ordine: progetto → note dei
pattern → metadati dei pattern → canali → arrangement (500 tracce) → `MixerParams` → 127 insert
(flag, 10 slot, colore, nome, routing, input/output). Plugin esterni VST3/CLAP diventano
"Fruity Wrapper" (vedi sopra) e le clip di automazione canali di tipo 5 dopo i canali normali.
Generatori senza equivalente FL e canali `ChannelKind::Automation` vuoti vengono scritti come
sampler vuoti con lo stesso nome. `from_project_report` restituisce anche gli avvisi.

## Limiti noti / incertezze

- Mappatura insert master/“current”: PyFLP e Monad.FLParser divergono; si segue Monad (master = 0).
- Muted dei clip (0x2000) e unità degli offset dei clip (tick per pattern, beat per audio) da
  Monad.FLParser; PyFLP li lascia non decodificati.
- Range di mix degli slot e dei livelli di mandata non verificati su file reali.
- `TrackData` esportato con i 48 byte documentati (FL 20.9 ne scrive 66).
- Clip di automazione FL senza collegamento nella convenzione descritta sopra, controller dei
  pattern, più arrangement, lunghezza dei pattern e note oltre MIDI 127 non sono importati
  (avviso). Lo stato `opaque_state` dei plugin nativi mappati
  viene riscritto tale e quale: modifiche ai parametri mfc non si riflettono in FL.
- L'export non è stato verificato aprendo i file in FL Studio.
