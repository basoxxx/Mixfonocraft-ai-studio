# Server MCP di Mixfonocraft

Mixfonocraft espone la DAW come server [Model Context Protocol](https://modelcontextprotocol.io)
sul trasporto stdio. Un agente AI può creare progetti, comporre pattern, mixare e fare
render usando gli stessi comandi della GUI e della CLI.

## 1. Avvio

```sh
mixfonocraft mcp [--workdir DIR] [--attach]
```

- `--workdir DIR`: cartella di lavoro. I percorsi relativi passati ai tool (`path`) vengono
  risolti rispetto a questa cartella; i percorsi assoluti vengono usati così come sono.
  Default: la directory corrente del processo.
- Il processo legge da stdin e scrive su stdout. **Su stdout passa solo il protocollo**: i
  messaggi diagnostici vanno su stderr.
- Il server termina quando stdin si chiude.
- `--attach`: invece di una sessione propria, si collega alla GUI aperta con il collegamento
  live attivo e inoltra i messaggi (vedi §7). Senza GUI in ascolto avvia la sessione headless.

## 2. Protocollo

### 2.1 Trasporto

JSON-RPC 2.0, un messaggio per riga (UTF-8, senza newline interni). Sono accettati anche
batch (array di richieste). Le notifiche (senza `id`) non ricevono risposta. Una riga che
non è JSON produce un errore `-32700` con `id: null`, e il server continua a leggere.

### 2.2 Versioni supportate

| Versione | Stato |
|---|---|
| `2025-06-18` | Preferita: è quella restituita se il client chiede una versione sconosciuta |
| `2025-03-26` | Supportata |
| `2024-11-05` | Supportata |

La capability dichiarata è `tools` con `listChanged: false`: il catalogo non cambia durante
la sessione.

### 2.3 Metodi

| Metodo | Risposta |
|---|---|
| `initialize` | `protocolVersion`, `capabilities`, `serverInfo` (`mixfonocraft`), `instructions` |
| `notifications/initialized` | Notifica, nessuna risposta |
| `ping` | `{}` |
| `tools/list` | Elenco dei 47 tool con `name`, `description` e `inputSchema` (JSON Schema) |
| `tools/call` | `{ "name": ..., "arguments": {...} }` → risultato del tool |
| `resources/list`, `prompts/list` | Liste vuote |

### 2.4 Risultati e errori

Un tool riuscito restituisce:

```json
{
  "content": [{"type": "text", "text": "...JSON formattato..."}],
  "isError": false,
  "structuredContent": { ... }
}
```

`structuredContent` è presente quando il risultato è un oggetto.

**Oggetti creati.** Ogni tool che crea qualcosa restituisce l'identificativo nel campo `id` al
primo livello, più i campi specifici:

| Tool | `id` | Altri campi |
|---|---|---|
| `channel_add`, `channel_clone` | canale | come `channel_list` (`name`, `plugin`, `volume`, `volume_db`, …) |
| `pattern_add`, `pattern_clone` | pattern | `pattern` (= `id`), `name`, `length_ticks`, `length_bars`, `notes` |
| `playlist_place` | prima clip | `clips` (tutte le clip create), `end` |
| `mixer_add_effect` | slot | `insert`, `slot`, `plugin`, `enabled`, `mix`, `params` |
| `automation_add` | automazione | `clip`: id della clip in Playlist se è stata piazzata, altrimenti `null` |
| `midi_import` | pattern | `pattern`, `channels`, `tempo` |
| `apply_commands` | — | `ids`: id creati, in ordine; `outcome`: esito grezzo | Un errore di esecuzione
(argomenti non validi, nota inesistente, comando rifiutato) **non** è un errore JSON-RPC: il
tool risponde con `isError: true` e il messaggio in `content`.

Gli errori JSON-RPC sono riservati a problemi del protocollo:

| Codice | Significato |
|---|---|
| `-32700` | JSON non valido |
| `-32600` | Richiesta non valida |
| `-32601` | Metodo non supportato |
| `-32602` | Parametro `name` mancante o tool sconosciuto |

### 2.5 Undo e stato

Ogni modifica passa da una sola funzione (`Session::exec`), che applica il comando con
undo e risincronizza il motore live se è attivo. Quindi `undo`, `redo` e `history` valgono
per qualunque tool che modifica il progetto. Il progetto resta in memoria finché non lo si
salva con `project_save`.

## 3. Tool

I nomi sono quelli del catalogo (`tools/list`). Gli argomenti marcati con `?` sono
opzionali. "Pattern opzionale" significa che, se omesso, si usa il pattern selezionato.

### 3.1 Progetto

| Tool | Descrizione | Argomenti principali |
|---|---|---|
| `project_new` | Crea un progetto vuoto (un pattern, mixer con 125 insert) | `title?`, `tempo?` |
| `project_open` | Apre `.mfc`, `.flp` o `.mid` (il MIDI diventa un nuovo progetto) | `path` |
| `project_save` | Salva in `.mfc` o, con estensione `.flp`, esporta in FL Studio | `path?` (default: percorso aperto) |
| `project_info` | Riepilogo: tempo, metrica, canali, pattern, durata, insert usati | — |
| `project_set` | Metadati, tempo, metrica, volume master, swing | `title?`, `artist?`, `genre?`, `comments?`, `tempo?`, `numerator?`, `denominator?`, `master_volume?` o `master_volume_db?`, `swing?` |
| `undo` | Annulla l'ultima modifica | — |
| `redo` | Ripristina l'ultima modifica annullata | — |
| `history` | Elenco delle modifiche annullabili, più recente prima | — |
| `apply_commands` | Esegue comandi grezzi di `mfc_core::Command` in un solo passo atomico di undo | `commands` (array di oggetti con `op`) |

`apply_commands` è l'accesso avanzato a tutto il modello. Ogni elemento è un comando
serializzato con `op` in snake_case, per esempio `{"op":"set_tempo","bpm":128}`.

### 3.2 Plugin e Channel Rack

| Tool | Descrizione | Argomenti principali |
|---|---|---|
| `plugin_list` | Generatori ed effetti nativi con parametri (nome, range, default, unità) | `role?` (`generator` o `effect`) |
| `channel_add` | Aggiunge un canale con un generatore | `name`, `plugin`, `insert?`, `params?`, `sample_path?` |
| `channel_list` | Elenca i canali | — |
| `channel_set` | Nome, volume, pan, pitch, mute, insert, nota radice, parametri, campione | `channel`, `name?`, `volume?` o `volume_db?`, `pan?`, `pitch_cents?`, `muted?`, `insert?`, `root_note?`, `params?`, `sample_path?` |
| `channel_remove` | Rimuove un canale con le sue note e clip audio | `channel` |
| `channel_clone` | Duplica un canale | `channel` |

Gli `params` sono valori reali (es. Hz, dB, ms); i nomi sono quelli restituiti da
`plugin_list`. Un nome sconosciuto produce un errore. I valori fuori range vengono bloccati
al range del parametro.

### 3.3 Pattern e step sequencer

| Tool | Descrizione | Argomenti principali |
|---|---|---|
| `pattern_add` | Crea un pattern vuoto e lo seleziona | `name` |
| `pattern_list` | Pattern con lunghezza e numero di note per canale | — |
| `pattern_select` | Seleziona il pattern corrente | `pattern` |
| `pattern_set` | Rinomina o imposta la lunghezza in battute (0 = automatica) | `pattern`, `name?`, `length_bars?` |
| `pattern_clone` | Duplica un pattern | `pattern` |
| `pattern_remove` | Elimina un pattern e le sue clip in Playlist | `pattern` |
| `steps_set` | Riga di step da 1/16 per un canale | `channel`, `row`, `pattern?` |

Caratteri di `steps_set`: `x` attivo (velocity 0.78), `X` accento (1.0), `o` soft (0.5),
`1` e `#` come `x`; `.` o `-` spento. Gli spazi e `|` sono ignorati, quindi `"x...x...|x...x..."`
è valido. La riga sostituisce solo le note dentro il suo intervallo.

### 3.4 Piano roll

| Tool | Descrizione | Argomenti principali |
|---|---|---|
| `notes_add` | Aggiunge note a un canale | `channel`, `notes` (array), `pattern?`, `replace?` |
| `notes_list` | Note di un pattern, opzionalmente di un canale | `pattern?`, `channel?` |
| `notes_remove` | Rimuove note con filtri su intervallo e tono | `channel`, `from?`, `to?`, `key?`, `pattern?` |
| `notes_quantize` | Quantizza posizioni (e opzionalmente durate) a una griglia in tick | `grid`, `channel?`, `quantize_length?`, `pattern?` |
| `notes_transpose` | Trasporta di N semitoni | `semitones`, `channel?`, `pattern?` |

Ogni elemento di `notes` ha:

- `position` (tick) **oppure** `step` (1/16, numero anche frazionario): obbligatorio uno dei due;
- `length` (tick, default 24 = uno step);
- `key`: numero MIDI oppure nome (`"C5"`, `"F#4"`, `"Bb3"`);
- `velocity` (0..=1, default 0.78), `pan` (-1..=1), `slide` (bool).

`replace: true` svuota prima le note del canale nel pattern.

### 3.5 Playlist

| Tool | Descrizione | Argomenti principali |
|---|---|---|
| `playlist_place` | Piazza un pattern, un'audio clip o un'automazione | `track`, `pattern` **oppure** `audio_channel` **oppure** `automation`; `position` o `bar`; `length` o `length_bars`; `repeat?` |
| `playlist_list` | Elenca le clip | — |
| `playlist_remove` | Rimuove una clip | `clip` |
| `playlist_move` | Sposta, ridimensiona o muta una clip | `clip`, `track?`, `position?`, `length?`, `muted?` |
| `playlist_track` | Rinomina o muta una traccia | `track`, `name?`, `muted?` |

`playlist_place` accetta esattamente una sorgente. Con `repeat: N` la clip viene ripetuta N
volte di seguito. La risposta restituisce gli id delle clip create e la fine dell'ultima.

### 3.6 Mixer ed effetti

| Tool | Descrizione | Argomenti principali |
|---|---|---|
| `mixer_list` | Insert in uso (o tutti con `all`): volume, pan, effetti, mandate | `all?` |
| `mixer_set` | Nome, volume, pan, separazione stereo, mute, solo, fase, scambio L/R (0 = master) | `insert`, `name?`, `volume?` o `volume_db?`, `pan?`, `stereo_separation?`, `muted?`, `solo?`, `phase_invert?`, `swap_lr?` |
| `mixer_route` | Crea, aggiorna o rimuove una mandata tra insert; i cicli sono rifiutati | `from`, `to`, `gain?`, `remove?` |
| `mixer_add_effect` | Aggiunge un effetto in uno slot (massimo 10 per insert) | `insert`, `plugin`, `params?`, `slot?` |
| `mixer_remove_effect` | Rimuove l'effetto da uno slot | `insert`, `slot` |
| `mixer_set_effect` | Abilita/disabilita, mix dry/wet, parametri di un effetto | `insert`, `slot`, `enabled?`, `mix?`, `params?` |

Il guadagno di una mandata è lineare, 0..=2. Ogni insert manda al master per default.
`mixer_set` restituisce l'insert aggiornato (come un elemento di `mixer_list`); `mixer_set_effect`
lo slot aggiornato.

**Sidechain.** Il compressore (`mfc.compressor`) ha il parametro `sidechain_insert`: `-1`
(default) usa il segnale dell'insert stesso; un numero `N ≥ 1` usa come chiave di rilevamento
l'uscita degli effetti dell'insert `N`, prima del suo fader. Quindi la chiave funziona anche
con un insert muto o a volume zero ("ghost kick"). Il motore elabora la sorgente prima
dell'insert compresso. Sono rifiutati il master (`0`), l'insert stesso e le sorgenti che
ricevono già il segnale dell'insert (ciclo). Esempio di ducking del basso (insert 2) sulla
cassa (insert 1):

```json
{"name":"mixer_add_effect","arguments":{"insert":2,"plugin":"mfc.compressor",
 "params":{"sidechain_insert":1,"threshold":-30,"ratio":8,"attack":1,"release":120}}}
```

### 3.7 Automazione

| Tool | Descrizione | Argomenti principali |
|---|---|---|
| `automation_add` | Crea una clip di automazione e, se richiesto, la piazza in Playlist | `name`, `target`, `min`, `max`, `points`, `track?`, `position?` |

`target` è un oggetto con `type`, per esempio `{"type":"master_volume"}`, `{"type":"tempo"}`,
`{"type":"channel_volume","channel":3}`, `{"type":"channel_param","channel":3,"param":"cutoff"}`,
`{"type":"mixer_volume","insert":1}`, `{"type":"effect_param","insert":1,"slot":0,"param":"wet"}`.
Ogni punto è `{"position": tick, "value": 0..1, "tension": -1..1 (opzionale)}`. Il valore
normalizzato viene mappato su `min..max`.

### 3.8 Render e transport

| Tool | Descrizione | Argomenti principali |
|---|---|---|
| `render_wav` | Esporta il brano (o un pattern) in WAV; restituisce durata, picco, RMS, stem | `path`, `pattern?`, `loops?`, `sample_rate?` (44100), `tail_seconds?` (2.0), `normalize?`, `stems?`, `format?` (`int16`, `int24`, `float32`; default `int24`) |
| `transport` | Transport live: play, stop, pause, seek, status; modalità song o pattern | `action`, `position?` (tick, per seek), `mode?` (`song` o `pattern`), `pattern?` |
| `preview_note` | Suona una nota di anteprima su un canale (attende la durata, max 5 s) | `channel`, `key`, `velocity?`, `duration_ms?` (300) |

Misure restituite da `render_wav`, per il mix e per ogni stem:

| Campo | Significato |
|---|---|
| `peak_db` | Picco campione (dBFS) |
| `true_peak_db` | Picco tra i campioni, sovracampionamento 4× (dBTP). Solo per il mix |
| `rms_db` | RMS su tutto il brano, silenzi compresi |
| `rms_active_db` | RMS solo sulle finestre di 50 ms sopra -60 dBFS: confronta i livelli di parti che suonano poco |
| `lufs_integrated` | Loudness integrata EBU R128 (LUFS), `null` se silenzioso |
| `gain_reduction_max_db` | Massima riduzione di guadagno di limiter e compressori sul master (dB, ≤ 0). Solo per il mix |
| `clipping` | Il mix supera 0 dBFS. Solo per il mix |

`render_wav` fallisce se la Playlist è vuota e non è indicato un `pattern`. Con gli stem,
i file sono scritti accanto al WAV principale, come `<nome>_<NNN>_<nome insert>.wav`, dove `NNN` è l'indice dell'insert (es. `beat_005_Lead.wav`).

Il transport live gira sul backend `realtime` se la build lo include e c'è una scheda audio;
altrimenti usa un backend senza uscita. Il campo `audible` della risposta indica quale dei
due è attivo.

### 3.9 MIDI e FL Studio

| Tool | Descrizione | Argomenti principali |
|---|---|---|
| `midi_import` | Importa un `.mid` in un nuovo pattern, un canale per traccia | `path`, `pattern_name?` |
| `midi_export` | Esporta un pattern, o l'intero brano se omesso | `path`, `pattern?` |
| `flp_import` | Importa un `.flp` sostituendo il progetto aperto; restituisce gli avvisi | `path` |
| `flp_export` | Esporta il progetto in `.flp` | `path` |
| `flp_inspect` | Eventi grezzi di un `.flp`, con paginazione e filtro per id | `path`, `offset?`, `limit?` (default 200), `id?` |

`flp_inspect` non modifica il progetto aperto: è utile per capire un file di FL Studio.
Vedi `docs/FLP_FORMAT.md` per il significato degli id.

## 4. Unità di misura

| Grandezza | Unità | Valori |
|---|---|---|
| Posizione e durata | **tick** (PPQ del progetto, default 96) | 1 beat = 96 tick; 1 step (1/16) = 24 tick; 1 battuta 4/4 = 384 tick |
| Posizione in Playlist | tick (`position`), oppure battute (`bar`, 0-based; `length_bars`) | `bar: 2` = tick 768 in 4/4 |
| Step | 1/16 di nota (`step`, `steps_set`) | 4 step per beat, 16 per battuta 4/4 |
| Tempo | BPM | 10..=999 |
| Fader canale | posizione (`volume`) o dB (`volume_db`) | `0..=1.25`; **0.78 = 0 dB**; massimo +4.1 dB |
| Fader insert e master | posizione (`volume`) o dB (`volume_db`) | `0..=1.25`; **0.8 = 0 dB**; massimo +3.9 dB |
| Pan | -1 (sinistra) .. 1 (destra) | 0 = centro |
| Velocity | 0..=1 | default 0.78 |
| Nota | numero MIDI o nome | **C5 = 60**; C4 = 48, A4 = 57, A2 = 33, C3 = 36 |
| Pitch canale | cent | -4800..=4800 |
| Automazione | `value` normalizzato | 0..=1, mappato su `min..max` del bersaglio |
| Mandata | guadagno lineare | 0..=2 (1 = unità) |
| Render | campioni al secondo | default 44100 |

**Curva dei fader.** Il fader è lineare in ampiezza: guadagno = `posizione / unità`, dove unità è
0.78 per i canali e 0.8 per insert e master. Quindi:

- dB = `20 · log10(posizione / unità)`;
- posizione = `unità · 10^(dB / 20)`.

Per un insert: 0.4 = -6.0 dB, 0.566 = -3.0 dB, 1.0 = +1.9 dB; da 0.55 a 0.42 si scende di 2.3 dB.
Conviene passare direttamente `volume_db` (≤ -120 dB porta il fader a zero); non si possono
passare `volume` e `volume_db` insieme. Le risposte riportano sempre entrambi i valori.

Il nome della nota segue la convenzione di FL Studio: l'ottava è `key / 12`, quindi C5 è 60 e
non 72. `key_name` restituisce la stessa notazione nelle risposte (`"name": "A2"`).

## 5. Esempio di sessione

Il file `examples/mcp/beat_session.jsonl` contiene una sessione completa, pronta da inviare
al server. Crea un beat di due battute a 128 BPM con kick, hats e basso, lo piazza in Playlist
su due ripetizioni, lo renderizza e salva il progetto:

```sh
mkdir -p /tmp/beat && mixfonocraft mcp --workdir /tmp/beat < examples/mcp/beat_session.jsonl
```

Ogni riga è una richiesta. Le più significative:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"project_new","arguments":{"title":"Beat di prova","tempo":128}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"channel_add","arguments":{"name":"Kick","plugin":"mfc.kick","insert":1}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"channel_add","arguments":{"name":"Hats","plugin":"mfc.drumsynth","params":{"type":3},"insert":3}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"channel_add","arguments":{"name":"Bass","plugin":"mfc.3xosc","insert":4,"params":{"cutoff":900,"release":0.12}}}}
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"pattern_set","arguments":{"pattern":1,"name":"Groove","length_bars":2}}}
{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"steps_set","arguments":{"channel":2,"row":"x...x...x...x...x...x...x...x..."}}}
{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"steps_set","arguments":{"channel":3,"row":"..x...x...x...x...x...x...x...x."}}}
{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"notes_add","arguments":{"channel":4,"notes":[{"step":0,"length":96,"key":"A2","velocity":0.85},{"step":8,"length":96,"key":"C3","velocity":0.8},{"step":16,"length":96,"key":"F2","velocity":0.85},{"step":24,"length":96,"key":"G2","velocity":0.8}]}}}
{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"playlist_place","arguments":{"track":0,"pattern":1,"bar":0,"repeat":2}}}
{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{"name":"render_wav","arguments":{"path":"beat.wav","sample_rate":44100,"tail_seconds":0.5,"format":"int24"}}}
{"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"project_save","arguments":{"path":"beat.mfc"}}}
```

Note sul flusso:

- Gli **id dei canali e delle clip** sono assegnati dal progetto e restituiti nella risposta
  di ogni `channel_add` e `playlist_place`. In questa sessione il primo pattern è `1`, i canali
  sono `2`, `3` e `4`, e le clip create sono `5` e `6`. Usare sempre gli id della risposta.
- `project_set` con `master_volume: 0.5` abbassa il master di circa 4 dB (0.5 rispetto allo
  0.8 di 0 dB). Senza questo passo il render di prova supera 0 dBFS (+2.1 dBFS, `clipping: true`).
- La risposta di `render_wav` riporta 8.0 s: 4 battute a 128 BPM (7.5 s) più 0.5 s di coda.
- Dopo `project_save` il file `beat.mfc` è nella cartella di `--workdir`.

Per annullare l'ultimo passo, un agente invia:

```json
{"jsonrpc":"2.0","id":16,"method":"tools/call","params":{"name":"undo","arguments":{}}}
```

## 6. Configurazione dei client

Per lavorare sul progetto aperto nella GUI aggiungere `--attach` agli argomenti (§7).

Claude Code:

```sh
claude mcp add mixfonocraft -- /percorso/a/mixfonocraft mcp --workdir /percorso/progetti
```

Claude Desktop, nel file di configurazione:

```json
{
  "mcpServers": {
    "mixfonocraft": {
      "command": "/percorso/a/mixfonocraft",
      "args": ["mcp", "--workdir", "/percorso/progetti"]
    }
  }
}
```

## 7. Collegamento live con la GUI

Di base `mixfonocraft mcp` e la GUI sono processi separati: l'agente lavora su un progetto in
memoria, che la GUI vede solo dopo il salvataggio. Con il collegamento live, invece, la GUI
espone il server MCP e l'agente lavora sul progetto aperto nella finestra.

1. Nella GUI: menu **Agente → Collegamento MCP live**, oppure avvio con `mixfonocraft-studio --mcp`
   o `mixfonocraft gui --mcp` (opzione `--workdir DIR` per la cartella dei percorsi relativi;
   default: la cartella del progetto aperto, poi `~/Music/Mixfonocraft`, poi la home).
2. Nel client: `mixfonocraft mcp --attach`. Il processo fa solo da ponte tra stdio e la GUI.

Comportamento:

- Ogni richiesta è eseguita sul thread della GUI, sulla stessa `Session::exec`: le modifiche
  compaiono subito, `undo` dell'agente e Ctrl+Z dell'utente usano la stessa cronologia, e il
  `transport` comanda il motore audio della finestra.
- La barra di stato mostra "MCP live" con il numero di chiamate servite.
- Se nessuna GUI è in ascolto, `--attach` scrive un avviso su stderr e avvia la sessione headless
  (con `--workdir`, come senza `--attach`).
- Se la GUI viene chiusa, il ponte termina: il client vede il server chiudersi.
- `preview_note` attende la durata della nota sul thread della GUI (massimo 5 s).

Endpoint:

| Sistema | Endpoint |
|---|---|
| macOS, Linux | Socket Unix con permessi 0600: `$XDG_RUNTIME_DIR/mixfonocraft.sock` se definita, altrimenti `<cartella temporanea>/mixfonocraft-<utente>.sock` |
| Windows | TCP su `127.0.0.1` con porta casuale; porta e token sono nel file `%TEMP%\mixfonocraft-live.txt` e il client invia il token come prima riga |

La variabile d'ambiente `MIXFONOCRAFT_SOCKET` sostituisce il percorso (su Windows, quello del
file di porta). Una sola GUI per volta può servire l'endpoint.
