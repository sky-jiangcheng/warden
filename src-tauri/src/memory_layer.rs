// The memory layer is Repotory's MCP server, not a store of the Agent's own.
//
// The positioning is fixed: the Agent is the entry point, Repotory is the
// memory, and MCP is the open protocol between them. This module answers the
// two questions the first screen asks about that memory layer: is the server
// installed here, and what can it actually do.
//
// Why there is NO tool list in this file. An earlier version carried a
// hardcoded `const TOOLS: &[&str]` with the tool names the Agent would
// drive. It had already drifted: it listed 7 names while Repotory's MCP
// surface actually registers 18, and the missing ones included
// `repotory_wiki_search` / `repotory_wiki_read` (the only way to read
// approved knowledge) and `repotory_ask` (evidence-backed answers). A
// hardcoded list is worse than no list: it looks authoritative, so a reader
// believes it is the contract.
//
// The single source of truth is Repotory's main repository, and the protocol
// already has the answer — MCP's `tools/list`. This module asks the server.
// The list in the payload is the server's answer, fetched over the wire, and
// it changes when the server changes.
//
// The two states are kept apart because they mean different things to a
// reader: "not installed" is an install problem the user can fix, and
// "installed but not answering" is a version or environment problem. One
// cannot be reported as the other.
use serde::Serialize;
use std::path::PathBuf;

use crate::binaries::find_binary;
use crate::mcp_client::{McpError, McpSession, ToolInfo};

/// The MCP server binary Repotory ships.
const MCP_BINARY: &str = "repotory-mcp";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryLayerStatus {
    /// Whether the MCP server is installed on this machine.
    pub available: bool,
    /// Where the MCP server lives, or None when it is not installed.
    pub path: Option<String>,
    /// Whether the server answered the handshake and gave its tool list.
    pub connected: bool,
    /// The tools the server advertises. Empty when it is not connected — an
    /// empty list here means "we do not know", never "the server has none".
    pub tools: Vec<ToolInfo>,
    /// Why the server could not be reached, when it is installed but not
    /// answering. None when it is not installed (that case is `available`).
    pub error: Option<String>,
}

/// Where the MCP server lives, or None when it is not installed.
pub fn detect_mcp() -> Option<PathBuf> {
    find_binary(MCP_BINARY)
}

/// Turns a client error into something a person can act on.
///
/// The raw error text mentions Rust types (`No such file or directory`,
/// `Broken pipe`); the screen needs the fact and the binary's location.
fn describe(err: &McpError) -> String {
    match err {
        McpError::Spawn(io) => format!("the MCP server could not be started: {io}"),
        other => other.to_string(),
    }
}

/// The status the first screen shows.
///
/// It connects to the server to get the tool list, because the list IS the
/// memory layer's capability — a probe that only finds the binary would say
/// "installed" while saying nothing about whether it works.
pub async fn status(session: &McpSession) -> MemoryLayerStatus {
    let Some(binary) = detect_mcp() else {
        return MemoryLayerStatus {
            available: false,
            path: None,
            connected: false,
            tools: Vec::new(),
            error: None,
        };
    };
    let path = Some(binary.display().to_string());

    match session.ensure().await {
        Ok((_client, tools)) => MemoryLayerStatus {
            available: true,
            path,
            connected: true,
            tools,
            error: None,
        },
        Err(err) => MemoryLayerStatus {
            available: true,
            path,
            connected: false,
            tools: Vec::new(),
            error: Some(describe(&err)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The status payload carries the tool list the SERVER reported, never a
    /// copy written down here. The negative assertions matter: a hardcoded
    /// list is what this module used to have, and it had already drifted from
    /// the server's real surface (7 names listed, 18 registered).
    ///
    /// The check is on the SOURCE, not on a runtime payload: the names would
    /// only show up in a payload if the server answered, and this test must
    /// hold without a server installed.
    ///
    /// Only STRING LITERALS are scanned, and only in the part above the test
    /// module. Two things fall outside on purpose: the comments, which quote
    /// tool names to explain why they must not be hardcoded, and the tests
    /// below, which quote one to prove the payload shape.
    #[test]
    fn no_tool_name_is_written_down_in_this_module() {
        let source = include_str!("memory_layer.rs");
        let code = source
            .split("#[cfg(test)]")
            .next()
            .expect("the module has a code part");
        for (number, line) in code.lines().enumerate() {
            let trimmed = line.trim_start();
            // A comment or a doc line: prose may name a tool to explain the
            // history. A string literal may not.
            if trimmed.starts_with("//") {
                continue;
            }
            let mut rest = trimmed;
            while let Some(at) = rest.find("\"") {
                rest = &rest[at + 1..];
                let end = rest.find("\"").expect("a closed string literal");
                let literal = &rest[..end];
                assert!(
                    !literal.contains("repotory_"),
                    "line {}: the tool name \"{literal}\" is written down.\n\
                     The list belongs to the server — fetch it over tools/list.",
                    number + 1
                );
                rest = &rest[end + 1..];
            }
        }
    }

    /// The binary's name is not a tool's name, and the two must not be
    /// confused: one is a file on disk, the other is a capability.
    #[test]
    fn the_binary_is_not_a_tool() {
        assert_eq!(MCP_BINARY, "repotory-mcp");
        assert!(!MCP_BINARY.contains("repotory_"));
    }

    /// A missing server is an install problem, and it says so — it is not
    /// reported as "installed but broken".
    #[test]
    fn a_missing_binary_is_reported_as_not_installed() {
        let status = serde_json::to_string(&MemoryLayerStatus {
            available: false,
            path: None,
            connected: false,
            tools: Vec::new(),
            error: None,
        })
        .expect("serialize");
        assert!(status.contains("\"available\":false"));
        assert!(status.contains("\"connected\":false"));
        assert!(status.contains("\"tools\":[]"));
        assert!(status.contains("\"error\":null"));
    }

    /// The three states are distinguishable in the payload: not installed,
    /// installed but not answering, and connected.
    #[test]
    fn the_three_states_are_distinguishable() {
        let connected = serde_json::to_string(&MemoryLayerStatus {
            available: true,
            path: Some("/usr/local/bin/repotory-mcp".to_string()),
            connected: true,
            tools: vec![ToolInfo {
                name: "repotory_context".to_string(),
                description: "ctx".to_string(),
            }],
            error: None,
        })
        .expect("serialize");
        assert!(connected.contains("\"connected\":true"));
        assert!(!connected.contains("\"connected\":false"));
        assert!(connected.contains("\"error\":null"));

        let broken = serde_json::to_string(&MemoryLayerStatus {
            available: true,
            path: Some("/usr/local/bin/repotory-mcp".to_string()),
            connected: false,
            tools: Vec::new(),
            error: Some("the MCP server did not answer in time (initialize)".to_string()),
        })
        .expect("serialize");
        assert!(broken.contains("\"available\":true"));
        assert!(broken.contains("\"connected\":false"));
        assert!(broken.contains("\"tools\":[]"));
        assert!(broken.contains("did not answer in time"));
    }
}
