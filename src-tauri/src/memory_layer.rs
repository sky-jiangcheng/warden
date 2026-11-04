// The memory layer is Repotory's MCP server, not a store of the Agent's own.
//
// The positioning is fixed: the Agent is the entry point, Repotory is the
// memory, and MCP is the open protocol between them. This module answers one
// question for the first screen — is that MCP server installed here, and
// where.
//
// Why there is NO tool list here. An earlier version carried a hardcoded
// `const TOOLS: &[&str]` with the tool names the Agent would drive. It had
// already drifted: it listed 7 names while Repotory's MCP surface actually
// registers 18, and the missing ones included `repotory_wiki_search` /
// `repotory_wiki_read` (the only way to read approved knowledge) and
// `repotory_ask` (evidence-backed answers). A hardcoded list is worse than no
// list: it looks authoritative, so a reader believes it is the contract.
//
// The single source of truth is Repotory's main repository, and the protocol
// already has the answer — MCP's `tools/list`. Phase 1 replaces this probe
// with a live client that asks the server. Until that client exists, the
// first screen says only what it actually knows: the binary is here (or it
// is not).
use serde::Serialize;
use std::path::PathBuf;

use crate::model_backends::find_binary;

/// The MCP server binary Repotory ships.
const MCP_BINARY: &str = "repotory-mcp";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryLayerStatus {
    /// Whether the MCP server is installed on this machine.
    pub available: bool,
    /// Where the MCP server lives, or None when it is not installed.
    pub path: Option<String>,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tool list used to be a hardcoded constant, and it drifted: Repotory
    /// registers 18 MCP tools while the copy listed 7 — including missing
    /// `repotory_wiki_search` / `repotory_wiki_read`, the only way to read
    /// approved knowledge. This test pins the shape that makes that
    /// impossible to repeat: the status payload carries NO tool list at all.
    ///
    /// The negative assertions matter here: a `tools` field coming back, or
    /// any `repotory_` name appearing in the payload, means someone re-added
    /// the hardcoded copy.
    #[test]
    fn status_carries_no_tool_list() {
        let payload = serde_json::to_string(&status()).expect("serialize");
        assert!(payload.contains("\"available\""));
        assert!(payload.contains("\"path\""));
        assert!(
            !payload.contains("tools"),
            "the status payload must not carry a tool list: it drifted before. \
             Fetch it over MCP's tools/list instead."
        );
        assert!(
            !payload.contains("repotory_"),
            "no tool names may be hardcoded here: the contract lives in Repotory's \
             main repository and is fetched over the protocol."
        );
    }
}
