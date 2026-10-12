//! Binario `mixfonocraft`: conversione, ispezione e render dei progetti, progetto
//! dimostrativo, server MCP e interfaccia grafica. La logica sta in `mfc_cli` (src/lib.rs).

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use mfc_cli::{
    build_demo, flp_inspect, load_project, new_project, parse_format, plugins_json, plugins_text, render_wav,
    save_project, Format, RenderRequest, Summary,
};
use mfc_core::{PatternId, Project};

#[derive(Parser)]
#[command(
    name = "mixfonocraft",
    version,
    about = "Mixfonocraft AI Studio: DAW clean-room in stile FL Studio",
    long_about = "Formati: .mfc (nativo JSON), .flp (FL Studio, import/export), .mid/.midi (MIDI).\n\
                  Esempi:\n  \
                  mixfonocraft demo demo.mfc --render demo.wav\n  \
                  mixfonocraft info demo.mfc\n  \
                  mixfonocraft convert demo.mfc demo.flp\n  \
                  mixfonocraft mcp --workdir ~/progetti"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Crea un progetto vuoto (un pattern, mixer con 125 insert).
    New {
        /// File di uscita (.mfc, .flp o .mid).
        out: PathBuf,
        /// Titolo del progetto.
        #[arg(long, default_value = "Untitled")]
        title: String,
        /// Tempo in BPM (10..=999, default 140).
        #[arg(long)]
        tempo: Option<f64>,
    },
    /// Riepilogo di un progetto: tempo, canali, pattern, Playlist, mixer.
    Info {
        /// Progetto da leggere (.mfc, .flp o .mid).
        file: PathBuf,
        /// Stampa il riepilogo come JSON.
        #[arg(long)]
        json: bool,
    },
    /// Converte un progetto tra formati (.mfc, .flp, .mid).
    Convert {
        /// Progetto di ingresso.
        input: PathBuf,
        /// Progetto di uscita; il formato è dall'estensione.
        output: PathBuf,
    },
    /// Renderizza il brano (o un pattern) in WAV.
    Render {
        /// Progetto da renderizzare.
        input: PathBuf,
        /// File WAV di uscita.
        output: PathBuf,
        /// Frequenza di campionamento in Hz.
        #[arg(long, default_value_t = 44_100)]
        sample_rate: u32,
        /// Renderizza solo questo pattern (id) invece del brano.
        #[arg(long)]
        pattern: Option<u32>,
        /// Ripetizioni del pattern (solo con --pattern).
        #[arg(long, default_value_t = 1)]
        loops: u32,
        /// Coda in secondi dopo la fine (riverberi, release).
        #[arg(long, default_value_t = 2.0)]
        tail: f64,
        /// Normalizza il picco a -0.3 dBFS.
        #[arg(long)]
        normalize: bool,
        /// Scrive anche uno stem per insert accanto al WAV principale.
        #[arg(long)]
        stems: bool,
        /// Formato dei campioni WAV.
        #[arg(long, default_value = "int24", value_parser = ["int16", "int24", "float32"])]
        format: String,
    },
    /// Elenca gli eventi grezzi di un file FL Studio (.flp).
    FlpInspect {
        /// File .flp.
        file: PathBuf,
        /// Mostra solo gli eventi con questo id.
        #[arg(long)]
        id: Option<u8>,
        /// Numero massimo di eventi mostrati.
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Salta i primi N eventi (dopo il filtro per id).
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Elenca i plugin nativi con i loro parametri.
    Plugins {
        /// Stampa il catalogo come JSON.
        #[arg(long)]
        json: bool,
    },
    /// Crea un brano dimostrativo (A minor, 128 BPM).
    Demo {
        /// Progetto di uscita (.mfc consigliato).
        out: PathBuf,
        /// Renderizza anche il brano in WAV.
        #[arg(long)]
        render: Option<PathBuf>,
    },
    /// Avvia il server MCP su stdin/stdout (solo protocollo JSON-RPC su stdout).
    Mcp {
        /// Cartella di lavoro per i percorsi relativi (default: directory corrente).
        #[arg(long)]
        workdir: Option<PathBuf>,
    },
    /// Apre la finestra grafica, opzionalmente con un progetto.
    #[cfg(feature = "gui")]
    Gui {
        /// Progetto da aprire (.mfc, .flp o .mid).
        file: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::New { out, title, tempo } => {
            let project = new_project(&title, tempo)?;
            save_project(&project, &out)?;
            println!("Creato {}", out.display());
        }
        Cmd::Info { file, json } => {
            let loaded = load_project(&file)?;
            for w in &loaded.warnings {
                eprintln!("avviso: {w}");
            }
            let summary = Summary::from_project(&loaded.project);
            if json {
                println!("{}", serde_json::to_string_pretty(&summary.to_json())?);
            } else {
                print!("{}", summary.to_text());
            }
        }
        Cmd::Convert { input, output } => {
            let loaded = load_project(&input)?;
            for w in &loaded.warnings {
                eprintln!("avviso: {w}");
            }
            save_project(&loaded.project, &output)?;
            println!(
                "Convertito {} → {} ({} canali, {} pattern, {} clip)",
                input.display(),
                output.display(),
                loaded.project.channels.len(),
                loaded.project.patterns.len(),
                loaded.project.playlist.clips.len()
            );
        }
        Cmd::Render {
            input,
            output,
            sample_rate,
            pattern,
            loops,
            tail,
            normalize,
            stems,
            format,
        } => {
            let loaded = load_project(&input)?;
            let req = RenderRequest {
                sample_rate,
                pattern: pattern.map(PatternId),
                loops,
                tail_seconds: tail,
                normalize,
                stems,
                format: parse_format(&format)?,
            };
            let report = render_wav(&loaded.project, &output, &req)?;
            println!("Scritto {}", report.path.display());
            println!("Durata:  {:.3} s", report.seconds);
            println!("Picco:   {:.1} dBFS", report.peak_db);
            println!("RMS:     {:.1} dBFS", report.rms_db);
            if report.clipping {
                println!("Attenzione: il mix supera 0 dBFS (clipping)");
            }
            for s in &report.stems {
                println!("Stem {:>3} {:<14} picco {:.1} dBFS", s.insert, s.name, s.peak_db);
            }
        }
        Cmd::FlpInspect {
            file,
            id,
            limit,
            offset,
        } => {
            let r = flp_inspect(&file, offset, limit, id)?;
            println!(
                "FLP formato {} · {} canali · PPQ {} · {} eventi ({} corrispondono al filtro)",
                r.format, r.channels, r.ppq, r.total, r.matching
            );
            println!("{:>6} {:>4} {:<28} {:<8} {:>7}  anteprima", "indice", "id", "nome", "tipo", "byte");
            for row in &r.rows {
                println!(
                    "{:>6} {:>4} {:<28} {:<8} {:>7}  {}",
                    row.index, row.id, row.name, row.kind, row.size, row.preview
                );
            }
        }
        Cmd::Plugins { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&plugins_json())?);
            } else {
                print!("{}", plugins_text());
            }
        }
        Cmd::Demo { out, render } => {
            let project: Project = build_demo()?;
            save_project(&project, &out)?;
            println!("Demo creato: {}", out.display());
            if let Some(wav) = render {
                let report = render_wav(&project, &wav, &RenderRequest::default())?;
                println!(
                    "Renderizzato {} ({:.2} s, picco {:.1} dBFS)",
                    report.path.display(),
                    report.seconds,
                    report.peak_db
                );
            }
        }
        Cmd::Mcp { workdir } => {
            let workdir = match workdir {
                Some(w) => w,
                None => std::env::current_dir().context("directory corrente non disponibile")?,
            };
            let mut server = mfc_mcp::Server::new(mfc_mcp::Session::new(workdir));
            server.run_stdio().context("errore del server MCP")?;
        }
        #[cfg(feature = "gui")]
        Cmd::Gui { file } => {
            let (project, path) = match file {
                Some(f) => {
                    let loaded = load_project(&f)?;
                    // Solo i .mfc possono essere salvati in place dalla GUI.
                    let keep = (Format::from_path(&f)? == Format::Mfc).then_some(f);
                    (loaded.project, keep)
                }
                None => (Project::new("Untitled"), None),
            };
            mfc_gui::run(project, path).map_err(|e| anyhow!("finestra grafica: {e}"))?;
        }
    }
    Ok(())
}
