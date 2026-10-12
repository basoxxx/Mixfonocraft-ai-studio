//! Adattatori tra i tool MCP e il crate `mfc-flp`.

use std::path::Path;

use mfc_core::Project;
use serde_json::{json, Value};

/// Importa un .flp: progetto + avvisi di mappatura.
pub fn import(path: &Path) -> Result<(Project, Vec<String>), String> {
    let (_, res) = mfc_flp::read_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((res.project, res.warnings))
}

/// Esporta il progetto in .flp.
pub fn export(path: &Path, project: &Project) -> Result<(), String> {
    mfc_flp::write_file(path, project).map_err(|e| format!("{}: {e}", path.display()))
}

/// Eventi grezzi di un .flp, paginati e filtrabili per id.
pub fn inspect(path: &Path, offset: usize, limit: usize, id: Option<u8>) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file = mfc_flp::parse(&bytes).map_err(|e| e.to_string())?;
    let all = mfc_flp::inspect(&file);
    let filtered: Vec<_> = all.iter().filter(|e| id.is_none_or(|x| e.id == x)).collect();
    let page: Vec<Value> = filtered
        .iter()
        .skip(offset)
        .take(limit.clamp(1, 5000))
        .map(|e| json!({"index": e.index, "id": e.id, "name": e.name, "kind": e.kind, "size": e.size, "preview": e.preview}))
        .collect();
    Ok(json!({
        "header": {"format": file.header.format, "channels": file.header.n_channels, "ppq": file.header.ppq},
        "total_events": all.len(),
        "matching": filtered.len(),
        "offset": offset,
        "events": page,
    }))
}
