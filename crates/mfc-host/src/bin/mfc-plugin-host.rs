//! `mfc-plugin-host`: processo helper che ospita un plugin VST3/CLAP fuori dall'app
//! (modalità sandbox). Vedi `mfc_host::sandbox`.

fn main() {
    std::process::exit(mfc_host::sandbox::server::run());
}
