# Piano: hosting VST3/CLAP e migliorie (v0.3.0)

## Obiettivo
Caricare plugin **VST3** e **CLAP** (strumenti ed effetti) nel Channel Rack e negli slot del
mixer, con **GUI native** dei plugin (macOS/Windows), stato salvato nel progetto, parametri
automatizzabili. In più: dialoghi file nativi + drag & drop, ingresso MIDI e tastiera del
computer con registrazione, impostazioni audio con carico CPU.

Decisioni (confermate dall'utente): GUI originali dei plugin; plugin in-process; CLAP oltre a VST3.

## Architettura

```
mfc-core      PluginInstance.instance_id (uid stabile delle istanze esterne)
mfc-plugins   external::{ExternalHost, set_external_host}: create_generator/create_effect
              delegano i kind "vst3:…" / "clap:…" all'host registrato
mfc-host      NUOVO. scan, PluginInfo, ExternalInstance, wrapper Generator/Effect,
              vst3/ (crate vst3), clap/ (clack-host), registry delle istanze vive
mfc-gui       browser plugin, editor parametri esterni, finestre native (plugin_window),
              dialoghi rfd, drag & drop, impostazioni audio, input MIDI/tastiera
mfc-engine    carico CPU, scelta dispositivo
```

**Kind**: `vst3:<percorso bundle>::<class id hex>` e `clap:<percorso file>::<plugin id>`.

**Istanze condivise**: costruire un plugin per ogni ricompilazione del motore è troppo costoso,
quindi `mfc-host` tiene un registro `instance_id → Arc<dyn ExternalInstance>`. Il motore
riceve un wrapper che condivide l'`Arc`; la GUI usa lo stesso `Arc` per editor e stato.

**Thread**: `process` (thread audio) usa `try_lock` sul processore; se occupato produce
silenzio, mai blocchi. Le modifiche ai parametri dalla GUI passano da una coda verso il thread
audio (VST3: `IParameterChanges`; CLAP: eventi param). Controller/GUI solo sul thread UI.

**Parametri**: nome = `"p<id>"`, valore normalizzato 0..1 (automazione e `set_param`).

**Stato**: al salvataggio la GUI chiede lo stato a ogni istanza viva (VST3 `getState`
component + controller; CLAP `state` ext) e lo scrive in `PluginInstance.opaque_state`;
al caricamento viene ripristinato.

## Verifica (TDD)
- Plugin di prova compilati nel workspace: `mfc-test-vst3` (synth sinusoidale + gain) e
  `mfc-test-clap` (stesso comportamento). I test d'integrazione li compilano, li impacchettano
  come bundle in una cartella temporanea e li caricano: scan → istanza → note → audio ≠ 0 →
  parametro gain → stato salvato/ripristinato → effetto in-place.
- GUI: screenshot headless del browser e dell'editor dei parametri esterni; le finestre native
  si verificano solo in compilazione (CI macOS/Windows).
- MIDI: test del parser/mapper e della registrazione con eventi sintetici.

## Lavoro e assegnazione
| # | Compito | Chi |
|---|---|---|
| 1 | core + hook `mfc-plugins` + scheletro `mfc-host` (tipi, trait, wrapper, registro) | Opus (io) |
| 2 | backend VST3 + plugin di prova + test | Opus (subagent) |
| 3 | backend CLAP + plugin di prova + test | Opus (subagent) |
| 4 | dialoghi nativi rfd, drag & drop, impostazioni audio, carico CPU | Haiku (subagent) |
| 5 | input MIDI (midir) e tastiera computer, registrazione nel Piano Roll | Opus (io) |
| 6 | integrazione GUI: browser plugin, editor esterni, finestre native, stato al salvataggio | Opus (io) |
| 7 | verifica finale, CI tre OS, release v0.3.0 | Opus (io) |
