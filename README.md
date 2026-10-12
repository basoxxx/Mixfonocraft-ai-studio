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

## Funzionalità

- **Channel Rack** con generatori nativi, canali sampler e audio clip, layer, pitch, pan e
  nota radice per canale.
- **Step sequencer** a 1/16 e **piano roll** con quantizzazione, trasposizione e velocity.
- **Playlist** su 500 tracce con clip di pattern, audio e automazione, ripetizioni e mute.
- **Mixer** con master e 125 insert, 10 slot effetto per insert, mandate, mute, solo, fase e
  scambio L/R, con 22 plugin nativi (8 generatori, 14 effetti).
- **Automazioni** su volume, pan, pitch, tempo, parametri di canale ed effetti, con curve a tensione.
- **Transport** in modalità Song o Pattern, con swing globale.
- **Render** offline in WAV (16 o 24 bit intero, 32 bit float), con normalizzazione e stem per insert.
- **Formati**: progetti nativi `.mfc` (JSON), import ed export `.flp`, import ed export MIDI (`.mid`).
- **Undo/redo** su ogni modifica, condiviso da GUI, CLI e server MCP.
- **CLI** `mixfonocraft` per convertire, ispezionare e renderizzare progetti, con un brano dimostrativo.
- **Server MCP** con 47 tool per creare progetti, pattern, mix e render da un agente AI.

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

| Piattaforma | File |
|---|---|
| macOS Apple Silicon (M1/M2/M3/M4) | `Mixfonocraft-<ver>-macos-arm64.dmg` / `.zip` |
| macOS Intel | `Mixfonocraft-<ver>-macos-x86_64.dmg` / `.zip` |
| Windows 10/11 x64 | `Mixfonocraft-<ver>-windows-x86_64.zip` |

Ogni pacchetto contiene l'app (doppio click = GUI), l'eseguibile `mixfonocraft` per CLI e
server MCP, il README e gli esempi. L'audio realtime è attivo (CoreAudio / WASAPI).

**macOS:** l'app è firmata solo ad-hoc (nessun certificato Apple Developer), quindi al primo
avvio Gatekeeper la blocca. Aprila con click destro → *Apri*, oppure:
`xattr -dr com.apple.quarantine /Applications/Mixfonocraft.app`.

**Windows:** SmartScreen può mostrare "PC protetto da Windows" → *Ulteriori informazioni* →
*Esegui comunque*. Lanciato senza argomenti, `mixfonocraft.exe` apre la GUI (resta visibile
anche una finestra console).

Per pubblicare una nuova versione: aggiornare `version` in `Cargo.toml`, poi
`git tag v0.1.0 && git push origin v0.1.0`.

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
mixfonocraft gui [progetto.mfc]
```

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
- I plugin nativi senza equivalente in FL Studio (drum synth, supersaw) diventano canali sampler
  vuoti con lo stesso nome nel `.flp`.
- L'audio realtime dipende dal backend della piattaforma; senza scheda audio il transport gira
  senza uscita.

## Licenza

Distribuito con licenza `MIT OR Apache-2.0`, come dichiarato nel `Cargo.toml` del workspace.
