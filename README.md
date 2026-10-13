# Mixfonocraft AI Studio

Mixfonocraft AI Studio è una DAW (Digital Audio Workstation) scritta in Rust, ispirata al
flusso di lavoro di FL Studio: Channel Rack con step sequencer, piano roll, Playlist, mixer a
125 insert con effetti e mandate, automazioni e interoperabilità con i file `.flp`. Un server
[MCP](https://modelcontextprotocol.io) permette a un agente AI di comporre e mixare un brano
usando gli stessi comandi della GUI.

Il progetto è una ricostruzione **clean-room**: il codice è scritto da zero e non deriva da
software Image-Line. Il formato `.flp` è stato mappato solo da documentazione pubblica della
community (PyFLP, Monad.FLParser e note sul formato), senza decompilazione. Mixfonocraft non è
affiliato né approvato da Image-Line; FL Studio è un marchio dei rispettivi titolari.

![Channel Rack](docs/screenshots/1_channel_rack.png)

| Piano Roll | Playlist |
|---|---|
| ![Piano Roll](docs/screenshots/2_piano_roll.png) | ![Playlist](docs/screenshots/3_playlist.png) |
| **Mixer** | **Editor plugin** |
| ![Mixer](docs/screenshots/4_mixer.png) | ![Editor plugin](docs/screenshots/5_plugin_editor.png) |

> Gli screenshot sono generati senza GPU dal rasterizzatore software incluso:
> `cargo run -p mfc-gui --release --features snapshot --example screenshot -- examples/demo.mfc docs/screenshots`

## Funzionalità

- **Channel Rack** con generatori nativi, canali sampler e audio clip, layer, pitch, pan e
  nota radice per canale.
- **Step sequencer** a 1/16 e **piano roll** con quantizzazione, trasposizione e velocity.
- **Playlist** su 500 tracce con clip di pattern, audio e automazione, ripetizioni e mute.
- **Mixer** con master e 125 insert, 10 slot effetto per insert, mandate, mute, solo, fase e
  scambio L/R, con 27 plugin nativi (10 generatori, 17 effetti), compressore con sidechain e
  volumi anche in dB.

  | Generatori | Effetti |
  |---|---|
  | Sampler, Audio Clip, 3xOsc, BooBass, Kick, Drum Synth, Supersaw, **FM2** (FM a 2 operatori), **Pluck** (corda Karplus-Strong), Silent | EQ, Compressor (sidechain), Limiter, Gate, Reverb, Delay, Chorus, Flanger, Phaser, Distortion, Bitcrusher, Filter, Stereo, Gain, **Ducker** (volume shaper a tempo), **Auto Pan** (pan/tremolo a tempo), **Transient Shaper** |
- **Automazioni** su volume, pan, pitch, tempo, parametri di canale ed effetti, con curve a tensione.
- **Transport** in modalità Song o Pattern, con swing globale.
- **Render** offline in WAV (16 o 24 bit intero, 32 bit float), con normalizzazione e stem per insert;
  misure di picco, true peak, RMS sulle parti attive, loudness EBU R128 (LUFS) e riduzione del limiter.
- **Formati**: progetti nativi `.mfc` (JSON), import ed export `.flp`, import ed export MIDI (`.mid`).
- **Undo/redo** su ogni modifica, condiviso da GUI, CLI e server MCP.
- **CLI** `mixfonocraft` per convertire, ispezionare e renderizzare progetti, con un brano dimostrativo.
- **Server MCP** con 47 tool per creare progetti, pattern, mix e render da un agente AI, anche
  **in diretta nella GUI** (collegamento live con undo condiviso).

## Struttura del workspace

| Crate | Ruolo |
|---|---|
| `mfc-core` | Modello del progetto, `Command` con undo/redo, tempo (PPQ, tick) |
| `mfc-dsp` | Primitive DSP real-time safe: oscillatori, filtri, inviluppi, delay, LFO |
| `mfc-plugins` | Generatori ed effetti nativi, descrittori dei parametri e registro |
| `mfc-engine` | Motore audio: sequencer sample-accurate, mixer, automazioni, render offline, backend |
| `mfc-flp` | Lettura e scrittura dei file FL Studio (`.flp`) |
| `mfc-midi` | Import ed export MIDI standard (SMF 0 e 1) |
| `mfc-mcp` | Server MCP su stdio e catalogo dei tool |
| `mfc-gui` | Interfaccia grafica egui: Browser, Channel Rack, Piano Roll, Playlist, Mixer, editor plugin |
| `mfc-cli` | Binario `mixfonocraft`: CLI, server MCP e GUI |

Per il dettaglio di dipendenze, modello dati e motore vedere [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Download (macOS e Windows)

I pacchetti pronti vengono generati dal workflow [`release.yml`](.github/workflows/release.yml)
e allegati alle [GitHub Release](../../releases) a ogni tag `v*` (per le pull request sono
disponibili come artifact della run):

| Piattaforma | File da scaricare |
|---|---|
| macOS Apple Silicon (M1/M2/M3/M4) | `Mixfonocraft-<ver>-macos-arm64.dmg` |
| macOS Intel | `Mixfonocraft-<ver>-macos-x86_64.dmg` |
| Windows 10/11 x64, installer | `Mixfonocraft-<ver>-windows-x86_64-setup.exe` |
| Windows 10/11 x64, MSI (per tutto il computer) | `Mixfonocraft-<ver>-windows-x86_64.msi` |
| Windows 10/11 x64, portatile (senza installazione) | `Mixfonocraft-<ver>-windows-x86_64-portable.zip` |

Ogni pacchetto contiene l'app Mixfonocraft AI Studio (doppio click = GUI, senza finestra console)
e l'eseguibile `mixfonocraft` per CLI e server MCP. L'audio realtime è attivo (CoreAudio / WASAPI).

**macOS**

1. Apri il `.dmg` e trascina **Mixfonocraft AI Studio** nella cartella **Applicazioni**.
2. Al primo avvio: l'app è firmata solo ad-hoc (nessun certificato Apple Developer), quindi
   Gatekeeper la blocca. Apri la cartella Applicazioni, fai click destro su *Mixfonocraft* →
   *Apri* → *Apri*. In alternativa, da terminale:
   `xattr -dr com.apple.quarantine /Applications/Mixfonocraft.app`

**Windows**

- **Setup.exe**: doppio click e segui la procedura. Puoi scegliere l'installazione per il solo
  utente o per tutto il computer, e se aggiungere la CLI al PATH e creare l'icona sul desktop.
  I file `.mfc` si aprono con Mixfonocraft AI Studio.
- **MSI**: installa in `Program Files` per tutto il computer (per l'installazione per utente usa Setup.exe).
- **Portatile**: estrai lo zip ed esegui `mixfonocraft-studio.exe`.
- SmartScreen può mostrare "PC protetto da Windows" → *Ulteriori informazioni* →
  *Esegui comunque* (il software non è firmato con certificato di codice).

**CLI e server MCP**

Il bundle macOS contiene due eseguibili in `Contents/MacOS/`:

| Eseguibile | Uso |
|---|---|
| `mixfonocraft-studio` | L'app: è quello che lanciano il doppio click, il Dock e `open -a Mixfonocraft` |
| `mixfonocraft` | CLI e server MCP (`mixfonocraft mcp …`): va indicato per percorso completo |

- macOS: `/Applications/Mixfonocraft.app/Contents/MacOS/mixfonocraft mcp --workdir /percorso/progetti`
- Windows: `mixfonocraft.exe` nella cartella di installazione (`C:\Program Files\Mixfonocraft AI Studio\`
  per l'MSI e per Setup.exe con installazione per tutto il computer,
  `%LOCALAPPDATA%\Programs\Mixfonocraft AI Studio\` per l'installazione per utente).
  Se nel PATH, basta `mixfonocraft` da terminale. Anche `mixfonocraft gui` apre la GUI.

Per pubblicare una nuova versione: aggiornare `version` in `Cargo.toml`, poi
`git tag v0.1.0 && git push origin v0.1.0`, oppure avviare manualmente il workflow *Release* (Actions → Release → Run workflow) indicando il tag.

## Compilazione

Serve un toolchain Rust recente (il workspace dichiara `rust-version = 1.80`).

```sh
cargo build --release -p mfc-cli
# binario: target/release/mixfonocraft
```

Feature di `mfc-cli`:

| Feature | Default | Effetto |
|---|---|---|
| `gui` | sì | Include la GUI (`mixfonocraft gui`) e le sue dipendenze grafiche |
| `realtime` | no | Uscita audio sulla scheda tramite `cpal`, per il transport live |

Su **Linux** la feature `realtime` richiede le librerie di sviluppo ALSA:

```sh
# Debian / Ubuntu
sudo apt install libasound2-dev
cargo build --release -p mfc-cli --features realtime
```

La feature `gui` usa eframe con backend OpenGL e supporto X11 e Wayland: serve un
driver grafico con OpenGL disponibile. Per una build senza interfaccia grafica, utile sui
server e per il solo server MCP:

```sh
cargo build --release -p mfc-cli --no-default-features
```

## Avvio rapido (CLI)

```sh
# Brano dimostrativo in A minor a 128 BPM, con render WAV
mixfonocraft demo demo.mfc --render demo.wav

# Riepilogo: tempo, canali, pattern, Playlist, mixer
mixfonocraft info demo.mfc
mixfonocraft info demo.mfc --json

# Conversione tra formati (.mfc, .flp, .mid)
mixfonocraft convert demo.mfc demo.flp
mixfonocraft convert demo.flp demo-da-flp.mfc

# Render del brano, di un pattern, con stem per insert
mixfonocraft render demo.mfc brano.wav --stems --format int16
mixfonocraft render demo.mfc groove.wav --pattern 1 --loops 4

# Ispezione degli eventi grezzi di un .flp
mixfonocraft flp-inspect progetto.flp --id 67 --limit 20

# Catalogo dei plugin con i parametri
mixfonocraft plugins
```

Nel repository c'è già un progetto dimostrativo in `examples/demo.mfc`.

Tutti i comandi: `mixfonocraft --help` e `mixfonocraft <comando> --help`.

## GUI

```sh
mixfonocraft gui [progetto.mfc] [--mcp] [--workdir DIR]
```

Un progetto `.mfc` si riapre con la vista (Playlist, Channel Rack…) e la modalità Song/Pattern
con cui è stato salvato. Un progetto senza questo stato (per esempio creato da un agente via MCP)
si apre in Song sulla Playlist se ha clip, altrimenti in Pattern sul Channel Rack, sul primo pattern.
Su macOS i file `.mfc`, `.flp` e `.mid` si aprono anche con doppio click nel Finder o con
`open -a Mixfonocraft file.mfc`, pure ad app già aperta (con conferma se ci sono modifiche non salvate).

Scorciatoie, in stile FL Studio:

| Tasto | Azione |
|---|---|
| `F5` | Playlist |
| `F6` | Channel Rack |
| `F7` | Piano Roll |
| `F9` | Mixer |
| `Spazio` | Play / stop |
| `L` | Alterna modalità Song e Pattern |
| `Ctrl+Z` | Annulla (`Cmd` su macOS) |
| `Ctrl+Y` | Ripristina |
| `Ctrl+S` | Salva (chiede il nome se il progetto non ha ancora un file) |

## Server MCP

Il server parla JSON-RPC 2.0 su stdin/stdout. I percorsi relativi dei tool vengono risolti
rispetto alla cartella indicata con `--workdir`.

Claude Code:

```sh
claude mcp add mixfonocraft -- /percorso/a/mixfonocraft mcp --workdir /percorso/progetti
```

Claude Desktop, in `claude_desktop_config.json`:

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

### Collegamento live con la GUI

Per vedere nella finestra le modifiche dell'agente mentre lavora, attiva il collegamento live
nella GUI (menu **Agente → Collegamento MCP live**, oppure avvia `mixfonocraft-studio --mcp` o
`mixfonocraft gui --mcp`) e registra il server con `--attach`:

```sh
claude mcp add mixfonocraft -- /percorso/a/mixfonocraft mcp --attach --workdir /percorso/progetti
```

Con `--attach` i tool lavorano sul progetto aperto nella finestra, con lo stesso undo (Ctrl+Z
annulla anche le modifiche dell'agente) e la stessa uscita audio. Se nessuna GUI è in ascolto,
`--attach` avvia la normale sessione headless. Dettagli in [`docs/MCP.md`](docs/MCP.md#7-collegamento-live-con-la-gui).

Protocollo, catalogo dei 47 tool, unità di misura ed esempio di sessione completa sono in
[`docs/MCP.md`](docs/MCP.md). Una sessione pronta da eseguire è in
`examples/mcp/beat_session.jsonl`.

## Test

```sh
cargo test -p mfc-cli                 # CLI: demo, render, conversioni, binario, server MCP
cargo test --workspace                # tutti i crate
cargo clippy -p mfc-cli --all-targets -- -D warnings
cargo fmt --all -- --check
```

I test di `mfc-cli` (`crates/mfc-cli/tests/cli.rs`) costruiscono il brano dimostrativo in
memoria, ne verificano struttura e render (durata, livello, dissolvenza), fanno il giro
`.mfc` → `.flp` → `.mfc` e `.mid`, eseguono il binario e dialogano con il server MCP via stdio.

## Documentazione

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): crate, modello dati, tempo, undo, motore audio, plugin, render, threading.
- [`docs/MCP.md`](docs/MCP.md): protocollo, tool, unità di misura, sessione di esempio.
- [`docs/FLP_FORMAT.md`](docs/FLP_FORMAT.md): mappatura del formato FL Studio.

## Limiti attuali

- Nessun hosting di plugin esterni (VST, CLAP): si usano solo i plugin nativi.
- L'esportazione `.flp` non è stata verificata con FL Studio reale. I test coprono il round-trip
  interno e la struttura del file.
- Le clip di automazione non vengono esportate nel `.flp`.
- I plugin FL sconosciuti vengono conservati come dati opachi e riesportati senza modifiche, ma
  non suonano.
- I plugin nativi senza equivalente in FL Studio (drum synth, supersaw, FM2, pluck) diventano canali sampler
  vuoti con lo stesso nome nel `.flp`.
- L'audio realtime dipende dal backend della piattaforma; senza scheda audio il transport gira
  senza uscita.

## Licenza

Distribuito con licenza `MIT OR Apache-2.0`, come dichiarato nel `Cargo.toml` del workspace.
