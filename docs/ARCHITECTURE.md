# Architettura di Mixfonocraft AI Studio

Questo documento descrive come è organizzato il workspace: i crate, il modello dati,
la gestione del tempo, il sistema di comandi con undo, il motore audio, il contratto dei
plugin, il render e le regole di threading. Riflette il codice presente nel repository;
dove una scelta è un compromesso, lo dichiara.

## 1. Principi

- **Un solo modello.** GUI, CLI e server MCP modificano il progetto esclusivamente tramite
  `mfc_core::Command`. Non esistono scorciatoie che aggirano il modello, quindi undo/redo,
  validazione e formato su disco sono uniformi.
- **Motore separato dal modello.** Il motore riceve un progetto già "compilato"
  (`Prepared`) e non legge mai il `Project` direttamente nel thread audio.
- **Clean-room.** Il modello è ispirato al workflow di FL Studio, ma non ne riproduce il
  codice. Il formato `.flp` è derivato solo da documentazione pubblica della community
  (vedi `docs/FLP_FORMAT.md`).

## 2. Grafo delle dipendenze

Ogni crate dipende solo da crate degli strati sottostanti (gli strati sono in ordine
dal basso verso l'alto):

```
  mfc-cli                              binario `mixfonocraft`
  mfc-mcp          mfc-gui             interfacce: server MCP su stdio, egui
  mfc-engine                           motore audio, render offline
  mfc-plugins                          generatori ed effetti nativi
  mfc-dsp          mfc-flp   mfc-midi  DSP; interoperabilità FL Studio e MIDI
  mfc-core                             modello del progetto, Command, History, tempo
```

| Crate | Dipende da | Ruolo |
|---|---|---|
| `mfc-core` | (nessuno interno) | Modello del progetto, `Command`, `History`, `TimeBase` |
| `mfc-dsp` | (nessuno interno) | Primitive DSP real-time safe: oscillatori, filtri, inviluppi, delay line, LFO, misuratori |
| `mfc-plugins` | `mfc-core`, `mfc-dsp` | Generatori ed effetti nativi, descrittori e `registry()` |
| `mfc-engine` | `mfc-core`, `mfc-plugins` | Motore audio, render offline, backend `null` e `cpal` (feature `realtime`) |
| `mfc-flp` | `mfc-core` | Lettura/scrittura `.flp`: eventi lossless, import e export semantici |
| `mfc-midi` | `mfc-core` | Import/export SMF (formati 0 e 1) |
| `mfc-mcp` | `mfc-core`, `mfc-plugins`, `mfc-engine`, `mfc-flp`, `mfc-midi` | Server MCP su stdio con 47 tool |
| `mfc-gui` | `mfc-core`, `mfc-plugins`, `mfc-engine`, `mfc-flp`, `mfc-midi` | Interfaccia egui/eframe |
| `mfc-cli` | `mfc-core`, `mfc-engine`, `mfc-flp`, `mfc-midi`, `mfc-mcp`, `mfc-plugins`, `mfc-gui` (opzionale) | Binario `mixfonocraft` |

`mfc-core` è il punto di convergenza: il modello è l'unico linguaggio comune tra GUI, CLI,
server MCP, motore e formati di file.

## 3. Modello dati

Il progetto (`mfc_core::Project`) riproduce il flusso di FL Studio:

```
Channel Rack ──► Pattern (step / piano roll) ──► Playlist ──► Mixer (master + 125 insert)
   canali           note per canale                clip           effetti, mandate, fader
```

- **Channel Rack**: lista di `Channel`. Ogni canale ha un `PluginInstance` (un generatore
  o un campione), un volume (fader), un pan, un pitch in cent, un mute, una nota radice
  (`root_note`, default 60 = C5) e un insert di destinazione nel mixer (`mixer_insert`).
  I canali di tipo `Layer` inoltrano le note ai figli.
- **Pattern**: `BTreeMap<ChannelId, Vec<Note>>`. Una `Note` ha posizione e durata in tick,
  tono MIDI (0..=127), velocity (0..=1), pan, fine pitch, release, slide e due controlli
  di modulazione. Lo step sequencer non ha una struttura propria: una riga di step è un
  insieme di note di durata uno step. La lunghezza è esplicita o calcolata dalle note,
  arrotondata alla battuta.
- **Playlist**: `Vec<PlaylistClip>` su 500 tracce. Una clip punta a un pattern, a un canale
  audio (`ClipSource::Audio`) o a una clip di automazione (`ClipSource::Automation`).
- **Mixer**: `Mixer` con `inserts[0]` = master e 125 insert, ciascuno con fader, pan,
  separazione stereo, mute/solo, inversione di fase, scambio L/R, fino a 10 slot effetto
  (`EffectSlot`, con mix dry/wet ed enable) e mandate (`Route`) verso altri insert.
  Di default ogni insert manda al master con guadagno 1.
- **Automazione**: `AutomationClip` con un `AutomationTarget` (volume master, tempo,
  volume/pan/pitch di canale, parametro di canale, volume/pan di insert, parametro o mix
  di effetto), un intervallo reale `min..max` e punti normalizzati `0..=1` con tensione
  della curva.

Gli id (`ChannelId`, `PatternId`, `ClipId`, `AutomationId`) sono numeri `u32` allocati
da `Project::alloc_id()`, che incrementa `next_id`. Sono stabili nel progetto e vengono
restituiti dai comandi che creano oggetti.

Il formato nativo `.mfc` è JSON serializzato con serde (`Project::to_json`/`from_json`),
con `format_version = 1`.

## 4. Tempo

- **PPQ** (`TimeBase::ppq`): tick per semiminima. Default 96, come i progetti FL nuovi.
- **Tick** (`Tick = u64`): unità di posizione e durata di tutto il modello.
- **Step**: 1/16 di nota, quindi `ppq / 4` tick (24 con PPQ 96).
- **Battuta**: `ppq * 4 / denominatore * numeratore` tick (384 in 4/4 con PPQ 96).
- **Conversioni**: `ticks_to_seconds` e `seconds_to_ticks` a tempo costante, e
  `samples_per_tick` per il motore. Il tempo è in BPM (10..=999).
- **Riscalatura**: l'import MIDI riscala le posizioni al PPQ del progetto
  (`parse_midi(bytes, target_ppq)`). L'import FLP mantiene i tick così come sono nel file
  (con il PPQ dell'header).
- **Swing**: globale (0..=1). Il motore ritarda gli step dispari fino a mezzo step, in
  proporzione al valore.

## 5. Comandi e undo

Ogni modifica è un `Command` (enum serde, campo `op` in snake_case). Le varianti coprono
progetto, Channel Rack, pattern, step, piano roll, Playlist, mixer, effetti e automazione.
`Command::Batch` raggruppa più comandi in un solo passo di undo.

`Command::apply(&mut Project)` esegue la modifica e valida gli input (range dei fader,
insert esistenti, nota 0..=127, metrica valida). In caso d'errore il progetto può restare
parzialmente modificato; per questo l'esecuzione passa da `History::execute`, che è atomica:

```
History::execute(project, cmd):
    snapshot = project.clone()       // stato precedente
    cmd.apply(project)?              // se fallisce: il progetto torna allo snapshot
    undo.push((cmd.label(), snapshot)); redo.clear()
```

Il `History` di default conserva 200 passi. L'undo è basato su snapshot: è semplice e
indipendente dal tipo di comando, al costo di clonare il progetto a ogni modifica. Per i
progetti attuali il costo è trascurabile; se diventasse un problema, il punto da cambiare
è solo `History`.

Le modifiche che non passano da `Command` (ad esempio il loro import MIDI) usano
`History::record(label, before)` per registrare lo snapshot.

## 6. Motore audio

### 6.1 Compilazione fuori dal thread audio

`Prepared::build(project, sample_rate)` trasforma un `Project` in uno stato pronto per il
thread audio:

- istanzia generatori ed effetti (caricando i campioni da disco);
- appiattisce le note di song e pattern in una sequenza di `NoteEvent` ordinata per `on`;
- calcola l'ordine topologico degli insert seguendo le mandate (algoritmo di Kahn). Il
  master è sempre l'ultimo. Eventuali cicli, possibili in progetti importati, vengono
  spezzati rimuovendo le mandate residue;
- calcola la maschera di ascolto per il solo, con la logica di FL: restano udibili l'insert
  in solo, ciò che lo alimenta e il percorso verso il master;
- risolve i bersagli di automazione in indici di canale/insert/effetto.

Questa fase alloca, quindi avviene sempre sul thread di controllo.

### 6.2 Comandi verso il motore

`Engine::new(project, sr)` restituisce il motore e un `EngineHandle`:

- l'handle invia `EngineCommand` (play, stop, pause, seek, modalità song/pattern, loop,
  `Install`, anteprima note) su un canale `crossbeam` limitato a 1024 messaggi; `try_send`
  non blocca mai, e se il canale è pieno il comando viene scartato con esito `false`;
- `EngineHandle::load_project` compila il progetto nel thread chiamante e invia
  `Install(Box<Prepared>)`;
- lo stato precedente torna indietro su un canale "garbage" e viene deallocato dal thread
  di controllo (`collect_garbage`), non da quello audio;
- lo stato condiviso (`SharedState`) usa atomici per posizione in tick, tempo, stato di
  riproduzione e picchi per insert e in uscita. La lettura del picco lo azzera.

`Install` riusa i generatori e gli effetti la cui chiave strutturale (`plugin_key`,
`channel_key`) non cambia: le voci in corso continuano senza interruzioni quando si modifica
un parametro o una nota.

### 6.3 Elaborazione

`Engine::process(left, right)` elabora il blocco in sotto-blocchi di al massimo `CHUNK = 64`
campioni:

1. applica le automazioni del sotto-blocco (`apply_automation`);
2. attiva gli eventi nota che cadono nell'istante corrente (`fire_events`);
3. accorcia il sotto-blocco fino al prossimo evento (`next_boundary`: note on/off attive,
   eventi della sequenza, fine del loop). Così note e automazioni cadono al campione giusto,
   senza un passo a dimensione fissa;
4. renderizza il sotto-blocco (`render_chunk`): i generatori sommano nei buffer del loro
   insert, poi gli insert vengono processati nell'ordine topologico con effetti, fader, pan,
   solo e mandate;
5. avanza la posizione in tick e gestisce il loop.

Le modalità sono `PlayMode::Song` (brano dalla Playlist) e `PlayMode::Pattern(id)` (loop di
un pattern, come il pulsante PAT). In Song, `set_loop` definisce un loop opzionale: è
disponibile nell'API del motore, ma al momento né la GUI né il server MCP lo espongono.

### 6.4 Backend

- `backend::null::NullBackend` esegue il motore su un thread a tempo reale che scarta
  l'audio. Serve per test, per il server MCP senza scheda audio e per le anteprime
  headless.
- `backend::device::DeviceBackend` (feature `realtime`, `cpal`) porta l'audio alla scheda.
  Su Linux richiede ALSA (`libasound2-dev` per la compilazione).

## 7. Contratto dei plugin

Definito in `crates/mfc-plugins/src/api.rs`.

- **Descrittore** (`PluginDescriptor`): `id` stabile (es. `"mfc.3xosc"`), `name`, `role`
  (`Generator` o `Effect`), `description`, e i `ParamDesc` dei parametri. Ogni parametro ha
  `name` (chiave snake_case in `PluginInstance::params`), `label`, `min`, `max`, `default`,
  `unit` e, per i parametri discreti, `choices`. I valori sono **reali** (es. Hz, dB, ms),
  non normalizzati; `normalize`/`denormalize`/`clamp` convertono quando serve.
- **Generatore** (trait `Generator`): `set_sample_rate`, `set_param`, `note_on(NoteOn)`,
  `note_off(voice_id)`, `all_notes_off`, `set_pitch_offset`, `process` (che **somma** nei
  buffer) e `active_voices`. `NoteOn` porta id della voce, tono, velocity, pan, microtuning,
  nota radice del canale, glide e offset di campione per le audio clip avviate a metà.
- **Effetto** (trait `Effect`): `set_sample_rate`, `set_param`, `reset`, `process` in-place,
  `latency`.
- **Registro**: `mfc_plugins::registry()` elenca i 23 descrittori nativi (9 generatori e 14
  effetti). `default_instance(id)` costruisce un'istanza con i default, `create_generator` e
  `create_effect` istanziano il DSP applicando i parametri.
- **Plugin sconosciuti**: un canale con un plugin non nativo diventa un generatore silenzioso.
  Un `PluginInstance` importato da `.flp` conserva il nome originale (`foreign_name`) e lo
  stato opaco (`opaque_state`), così viene riesportato senza perdite.

Regole real-time per l'implementazione: `process` non deve allocare né bloccare. Le
allocazioni (buffer, delay line, campioni) avvengono nel costruttore o in `set_sample_rate`.

## 8. Automazione

Una clip di automazione mappa i punti `value` (0..=1) sul range `min..max` del bersaglio.
Tra due punti la curva è lineare oppure segue una tensione `tension` (`apply_tension`, curva
esponenziale). Il motore calcola il valore a ogni sotto-blocco e lo applica al bersaglio:

- `MasterVolume` imposta il guadagno master con `fader_gain(v, MIXER_UNITY)`;
- `Tempo` modifica il tempo di riproduzione (il render tronca la coda a fine brano solo se
  non c'è automazione di tempo).

Nel file FLP le clip di automazione non vengono esportate (vedi sezione 10).

## 9. Render e stem

`mfc_engine::render(project, &RenderOptions)` esegue il motore in modalità offline:

- la durata è fissata dalla sequenza (brano o pattern × ripetizioni), tagliata con precisione
  alla fine musicale;
- la coda (`tail_seconds`) fa girare il motore fermo per far morire riverberi e release;
- `max_seconds` (default 30 minuti) è un limite di sicurezza;
- con `stems = true` il motore cattura il segnale post-fader di ogni insert con segnale,
  tagliato allo stesso punto del mix;
- `normalize = true` riporta il picco a -0.3 dBFS.

`render_to_file` scrive il mix in WAV (`Int16`, `Int24` o `Float32`) e, se richiesto, gli
stem come `<base>_<NNN>_<nome>.wav` accanto al file principale.

## 10. Interoperabilità

- **FLP** (`mfc-flp`): tre livelli. `parse`/`write` fanno round-trip byte-identico dei
  file ben formati; `inspect` riassume gli eventi; `to_project` e `from_project` mappano
  verso il modello. Le mappature e le limitazioni sono in `docs/FLP_FORMAT.md`. In
  esportazione, i generatori nativi con un equivalente FL (3x Osc, Fruity Kick) diventano
  plugin FL; gli altri diventano canali sampler vuoti con lo stesso nome; le clip di
  automazione non vengono esportate.
- **MIDI** (`mfc-midi`): `parse_midi` legge SMF 0 e 1 (timing metrico, non SMPTE) e
  riscala al PPQ del progetto; `import_into_project` crea un pattern e un canale per traccia
  con comandi (quindi annullabili). I canali MIDI 10 usano `mfc.drumsynth`, gli altri
  `mfc.3xosc`. `export_pattern` ed `export_song` scrivono SMF di formato 1.

## 11. Server MCP

`mfc-mcp` implementa JSON-RPC 2.0 su stdio (un messaggio per riga). `Server` gestisce
`initialize`, `ping`, `tools/list`, `tools/call`, `resources/list` e `prompts/list` (vuoti).
Il catalogo dei tool è in `tools.rs`: ogni tool è un wrapper sottile su `Command` o su un
servizio (render, transport, import/export). Ogni modifica passa da `Session::exec`, che
applica il comando con undo e risincronizza il motore live, se attivo.

I dettagli dei tool e del protocollo sono in `docs/MCP.md`.

## 12. Threading e regole real-time

- Il **thread audio** esegue solo `Engine::process` e `handle_commands`. Non alloca, non
  prende lock e non fa I/O. I comandi arrivano da un canale limitato; i `Prepared` dismessi
  tornano indietro per essere deallocati altrove.
- I **thread di controllo** (GUI, MCP, CLI) costruiscono i `Prepared`, leggono i picchi e
  mandano comandi. `EngineHandle::send` raccoglie prima i `Prepared` dismessi.
- Il **modello** (`Project`) non è condiviso con il thread audio: il motore riceve sempre
  una copia compilata. Questo evita lock sul progetto durante l'elaborazione.
- Il **server MCP** è single-threaded: ogni richiesta è gestita in ordine sulla sessione.
  Il solo tool che attende è `preview_note`, con una pausa limitata a 5 secondi.
- Il **render offline** gira sul thread chiamante e non richiede un backend audio.

## 13. Limiti noti

- Nessun hosting di plugin esterni (VST, CLAP): solo i plugin nativi di `mfc-plugins`.
- L'esportazione FLP non è stata verificata con FL Studio reale; i test coprono il
  round-trip interno e la struttura dei file.
- Le clip di automazione non sono mappate nel formato FLP.
- I plugin FL sconosciuti sono preservati come blob opachi e riesportati senza modifiche, ma
  non suonano.
- I plugin nativi senza equivalente FL vengono esportati come canali sampler vuoti.
- L'undo è basato su snapshot: semplice, ma con costo proporzionale alla dimensione del
  progetto per ogni modifica.
