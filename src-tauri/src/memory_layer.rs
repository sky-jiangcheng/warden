// The memory layer is Repotory's MCP server, not a store of the Shell's own.
//
// The positioning is fixed: the Shell is the entry point, Repotory is the
// memory, and MCP is the open protocol between them. This module answers one
// question for the first screen — is that MCP server installed here, and
// where — plus the tool names the Shell will drive in Phase 1.
//
// The tool list is the actual contract of today's Repotory MCP. Phase 1 turns
// this detection into a live MCP client (connect, call repotory_context,
// write repotory_handoff); Phase 0 only reports, so the UI can say whether
// the memory layer is even installed before it promises anything.
use serde::Serialize;
use std::path::PathBuf;

use crate::model_backends::find_binary;

/// The MCP server binary Repotory ships.
const MCP_BINARY: &str = "repotory-mcp";

/// The tools the Shell drives. These names exist in Repotory's MCP today —
/// a Shell that invented its own would be a second memory API.
const TOOLS: &[&str] = &[
    "repotory_context",
    "repotory_handoff",
    "repotory_notes_search",
    "repotory_notes_create",
    "repotory_notes_list",
    "repotory_projects_list",
    "repotory_scan",
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryLayerStatus {
    /// Whether the MCP server is installed on this machine.
    pub available: bool,
    pub path: Option<String>,
    /// The tools the Shell will drive over MCP.
    pub tools: Vec<String>,
}

/// Where the MCP server lives, or None when it is not installed.
pub fn detect_mcp() -> Option<PathBuf> {
    find_binary(MCP_BINARY)
}

pub fn status() -> MemoryLayerStatus {
    let found = detect_mcp();
    MemoryLayerStatus {
        available: found.is_some(),
        path: found.map(|p| p.display().to_string()),
        tools: TOOLS.iter().map(|t| t.to_string()).collect(),
    }
}
